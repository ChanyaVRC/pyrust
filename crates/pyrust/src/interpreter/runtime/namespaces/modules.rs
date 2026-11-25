struct FilesystemModuleTarget {
    file_path: PathBuf,
    package_dir: Option<PathBuf>,
}

/// Resolved execution target for `pyrust -m <module>`.
pub(crate) struct MainModuleTarget {
    pub(crate) file_path: PathBuf,
    pub(crate) package_name: String,
}

/// Failure modes while resolving `pyrust -m <module>`.
pub(crate) enum MainModuleResolveError {
    /// CLI-level message printed as `<prog>: <message>` (exit 1).
    Message(String),
    /// Exception raised while importing a parent or target package's
    /// `__init__.py` (or a `SystemExit` raised during import), along with any
    /// captured traceback frames.
    ImportException(PyError, Vec<pyrust_core::FrameInfo>),
}

/// Locate a filesystem package (`<dir>/<leaf>/__init__.py`) or module
/// (`<dir>/<leaf>.py`) across `search_dirs`.
///
/// Within each search directory, a package takes precedence over a sibling
/// `.py` module, matching CPython 3.12's `FileFinder`. Across directories,
/// earlier entries in `search_dirs` win.
fn find_filesystem_module(
    search_dirs: &[PathBuf],
    leaf_name: &str,
) -> Option<FilesystemModuleTarget> {
    for dir in search_dirs {
        let pkg_dir = dir.join(leaf_name);
        let init_path = pkg_dir.join("__init__.py");
        if init_path.is_file() {
            return Some(FilesystemModuleTarget {
                file_path: init_path,
                package_dir: Some(pkg_dir),
            });
        }
        let mod_path = dir.join(format!("{leaf_name}.py"));
        if mod_path.is_file() {
            return Some(FilesystemModuleTarget {
                file_path: mod_path,
                package_dir: None,
            });
        }
    }
    None
}

// Generic module loading and lookup.
fn validate_import_cache_entry(name: &str, cached: Option<Value>) -> Result<Option<Value>> {
    if cached.as_ref().is_some_and(Value::is_none) {
        return Err(PyError::import_error(
            "ModuleNotFoundError",
            format!("import of {name} halted; None in sys.modules"),
            Some(name.to_string()),
        ));
    }
    Ok(cached)
}

impl Interpreter {
    /// Resolve a module-owned class through a typed cache slot.
    ///
    /// The steady-state path is three shared-generation reads plus a weak class
    /// upgrade. A `sys.modules` replacement/reload or a module attribute
    /// monkey-patch invalidates the corresponding generation and falls back to
    /// the authoritative import and attribute lookup path. Exhausted registry
    /// or module generations remain permanently uncacheable.
    #[inline(always)]
    pub(crate) fn cached_module_class(
        &mut self,
        slot: ModuleClassCacheSlot,
        module_name: &str,
        attribute_name: &str,
    ) -> Result<Rc<RefCell<PyClass>>> {
        debug_assert!(slot.0 < MODULE_CLASS_CACHE_SLOT_COUNT);
        if let Some(entry) = self
            .cold
            .module_class_cache
            .as_ref()
            .and_then(|cache| cache.entries.get(slot.0))
            .and_then(Option::as_ref)
            && entry
                .registry_owner_state
                .matches_cache_version(entry.registry_owner_version)
            && entry
                .registry_state
                .matches_cache_version(entry.registry_version)
            && entry
                .module_state
                .matches_cache_version(entry.module_version)
            && let Some(class) = entry.class.upgrade()
        {
            return Ok(class);
        }

        self.resolve_module_class_cache_miss(slot, module_name, attribute_name)
    }

    /// Resolve and refresh a stale module-class cache entry.
    ///
    /// Imports, Python-visible namespace reads, and cache allocation belong to
    /// the miss path. Keeping them out of [`Self::cached_module_class`] lets the
    /// ordinary alias-construction path inline to three generation checks and a
    /// weak upgrade without weakening reload or monkey-patch invalidation.
    #[cold]
    #[inline(never)]
    fn resolve_module_class_cache_miss(
        &mut self,
        slot: ModuleClassCacheSlot,
        module_name: &str,
        attribute_name: &str,
    ) -> Result<Rc<RefCell<PyClass>>> {
        // Resolve the authoritative module before gathering optional cache
        // guards. `load_module` intentionally gives an entry retained in the
        // interpreter's original registry priority over the Python-visible
        // `sys.modules` attribute. A missing or invalid visible attribute must
        // therefore not turn that successful internal hit into an error.
        let module_value = self.load_module(module_name)?;
        let ValueKind::PyModule(module) = module_value.kind() else {
            return Err(PyError::runtime(format!(
                "{module_name}: active module is not a module"
            )));
        };
        let (module_state, class) = {
            let module = module.borrow();
            let class = module.get_attr_value(attribute_name).ok_or_else(|| {
                PyError::runtime(format!("{module_name}: missing {attribute_name}"))
            })?;
            let ValueKind::PyClass(class) = class.kind() else {
                return Err(PyError::runtime(format!(
                    "{module_name}: {attribute_name} is not a class"
                )));
            };
            (module.mutation_state(), Rc::clone(class))
        };

        // Cache installation is best-effort metadata collection after the
        // semantic lookup. The existing guards cover the live `sys.modules`
        // binding and one exact registry. A replacement, missing, or invalid
        // binding stays uncached: such states either need two independent
        // content guards or have no usable mutation generation.
        let cache_entry = self
            .import_module_registry_with_owner_state()
            .ok()
            .and_then(|(registry, registry_owner_state)| {
                if !values_are_identical(&registry, &self.cold.bootstrap_module_registry) {
                    return None;
                }
                let registry_owner_state = registry_owner_state?;
                let registry_state = registry.dict_iteration_mutation_state()?;
                let module_version = module_state.cache_version()?;
                let registry_owner_version = registry_owner_state.cache_version()?;
                let registry_version = registry_state.cache_version()?;
                Some(CachedModuleClass {
                    registry_owner_version,
                    registry_owner_state,
                    registry_version,
                    registry_state,
                    module_version,
                    module_state,
                    class: Rc::downgrade(&class),
                })
            });
        if let Some(entry) = cache_entry {
            self.cold
                .module_class_cache
                .get_or_insert_with(|| Box::new(ModuleClassCache::default()))
                .entries[slot.0] = Some(entry);
        } else if let Some(cache) = self.cold.module_class_cache.as_mut() {
            cache.entries[slot.0] = None;
        }
        Ok(class)
    }

    /// Register a freshly loaded module in this interpreter's `sys.modules`
    /// (issue #2727), keyed by its dotted import name.  CPython exposes every
    /// imported module here as the import cache; pyrust mirrors each
    /// `module_cache` insertion into the active Python-visible registry so user
    /// code observes membership and identity.
    fn register_in_sys_modules(&mut self, name: &str, module: &Value) -> Result<()> {
        self.insert_import_registry(name, module.clone())
    }

    /// Remove a module whose initialization raised from both import caches.
    ///
    /// This applies equally to filesystem modules and built-in modules whose
    /// Python-source injection/finalization failed.  Leaving the initially
    /// registered object behind would make the next `import` return a
    /// half-initialized module instead of retrying its body.
    fn unregister_failed_module(&mut self, name: &str) {
        self.cold.module_cache.borrow_mut().remove(name);
        let _ = self.remove_import_registry(name);
    }

    /// Look up `name` in the user-visible `sys.modules` dict (issue #2727).
    /// This is the importlib-visible cache in CPython: a direct write makes a
    /// previously unknown name a cache hit, while deleting a name not retained
    /// in the interpreter's original dict forces re-execution. Existing entries
    /// in that original dict are resolved before this mapping by
    /// [`Self::load_module`].
    fn lookup_sys_modules(&mut self, name: &str) -> Result<Option<Value>> {
        let cached = self.lookup_import_registry(name)?;
        validate_import_cache_entry(name, cached)
    }

    /// Populate the per-interpreter `sys.path` when `sys` is first imported.
    /// The generated built-in module constructor has no access to the active
    /// script, so the initial import root cannot live in `bodies/sys.rs`.
    pub(crate) fn initialize_system_module(&self, module: &Value) {
        let ValueKind::PyModule(sys) = module.kind() else {
            return;
        };
        let initial_path = self
            .cold
            .script_dir
            .as_ref()
            .map(|dir| Value::string(dir.to_string_lossy()))
            .unwrap_or_else(|| Value::string(""));
        let mut sys = sys.borrow_mut();
        // Seed the module slots at the *front* of the still-ordered `attrs`
        // before it is moved into the live dict, so `list(vars(sys))` starts
        // with CPython's five module-object slots in CPython's order — the
        // same head the synthesised built-in `__dict__` produces for a module
        // that keeps direct `attrs` storage (issue #2918). Appending them
        // after `attach_live_namespace` put them at the tail instead.
        for (index, (name, value)) in [
            ("__name__", Value::string("sys")),
            ("__doc__", Value::none()),
            ("__package__", Value::string("")),
            ("__loader__", Value::none()),
            ("__spec__", Value::none()),
        ]
        .into_iter()
        .enumerate()
        {
            sys.attrs.shift_insert(index, name.to_string(), value);
        }
        sys.attach_live_namespace();
        sys.insert_attr("path".to_string(), Value::list(vec![initial_path]));
        sys.insert_attr(
            "modules".to_string(),
            self.cold.bootstrap_module_registry.clone(),
        );
        // Fresh per-interpreter mutable sys containers and aliases (#2970).
        let orig_argv: Vec<Value> = self.cold.script_argv.iter().map(Value::string).collect();
        sys.insert_attr("orig_argv".to_string(), Value::list(orig_argv));
        sys.insert_attr("warnoptions".to_string(), Value::list(Vec::new()));
        sys.insert_attr("meta_path".to_string(), Value::list(Vec::new()));
        sys.insert_attr("path_hooks".to_string(), Value::list(Vec::new()));
        sys.insert_attr(
            "path_importer_cache".to_string(),
            Value::dict(PyDict::default()),
        );
        sys.insert_attr("_xoptions".to_string(), Value::dict(PyDict::default()));
        // Keep `sys.__stdout__ is sys.stdout` (and stderr/stdin/hooks) exact.
        for (src, alias) in [
            ("stdout", "__stdout__"),
            ("stderr", "__stderr__"),
            ("stdin", "__stdin__"),
            ("displayhook", "__displayhook__"),
            ("excepthook", "__excepthook__"),
            ("unraisablehook", "__unraisablehook__"),
            ("breakpointhook", "__breakpointhook__"),
        ] {
            if let Some(val) = sys.get_attr_value(src) {
                sys.insert_attr(alias.to_string(), val);
            }
        }
    }

    /// Snapshot the import roots currently exposed through `sys.path`.
    /// Before `sys` is imported, preserve the legacy script-directory lookup
    /// so a script can still import a sibling module as its very first import.
    fn user_module_search_dirs(&self) -> Vec<PathBuf> {
        let sys_path = self
            .cold
            .module_cache
            .borrow()
            .get("sys")
            .and_then(|module| {
                let ValueKind::PyModule(sys) = module.kind() else {
                    return None;
                };
                sys.borrow().get_attr_value("path")
            });

        if let Some(path) = sys_path
            && let ValueKind::List(entries) = path.kind()
        {
            return entries
                .iter()
                .filter_map(|entry| entry.as_str().map(PathBuf::from))
                .collect();
        }

        self.cold.script_dir.iter().cloned().collect()
    }

    /// Notify all registered `sys.addaudithook` callbacks of an audit event.
    pub(crate) fn emit_audit_event(&mut self, event: &str, event_args: &[Value]) -> Result<()> {
        let hooks: Vec<Value> = self.sys_state.borrow().audit_hooks.clone();
        if hooks.is_empty() {
            return Ok(());
        }
        let call_args = [
            ExpandedCallArg {
                name: None,
                value: Value::string(event),
            },
            ExpandedCallArg {
                name: None,
                value: Value::tuple(event_args.to_vec()),
            },
        ];
        for hook in hooks {
            self.call_function_expanded(hook, &call_args)?;
        }
        Ok(())
    }

    /// Read a module-scope dunder (`__package__`, `__name__`, `__path__`) from
    /// the active defining module's root namespace without forcing a globals
    /// alias exposure on fastlocal scripts.
    fn lookup_active_module_global(&self, name: &str) -> Option<Value> {
        let namespace = self.prepare_global_namespace(name);
        let root = module_env(&self.env);
        let root_ref = root.borrow();
        if let Some(val) = root_ref.namespace_fastlocal_value(name) {
            return (!val.is_unset()).then_some(val);
        }
        if let Some(val) = root_ref.namespace_values().get(name).cloned() {
            return (!val.is_unset()).then_some(val);
        }
        namespace
            .globals()
            .dict_with(|dict| dict.get(&StrKey(name)).cloned())
            .flatten()
            .filter(|val| !val.is_unset())
    }

    /// Resolve a relative import `(rel_name, level)` (`level >= 1`) against the
    /// importing module's `__package__`, matching CPython 3.12's
    /// `importlib._bootstrap._calc___package__` and `_resolve_name`.
    pub(crate) fn resolve_relative_module_name(
        &self,
        rel_name: &str,
        level: usize,
        globals_override: Option<&Value>,
    ) -> Result<String> {
        let lookup_key = |key: &str| -> Option<Value> {
            if let Some(globals) = globals_override {
                globals
                    .dict_with(|dict| dict.get(&StrKey(key)).cloned())
                    .flatten()
                    .filter(|val| !val.is_unset())
            } else {
                self.lookup_active_module_global(key)
            }
        };

        let package_val = lookup_key("__package__");
        let package = match package_val {
            Some(val) if !val.is_none() => match val.as_str() {
                Some(s) => s.to_string(),
                None => {
                    return Err(PyError::named(
                        "TypeError",
                        "package must be a string".to_string(),
                    ));
                }
            },
            _ => {
                // Fallback matching CPython `_calc___package__`: when
                // `__package__` is `None` or absent, infer from `__name__` and
                // `__path__`.
                if let Some(name_str) =
                    lookup_key("__name__").and_then(|val| val.as_str().map(str::to_string))
                {
                    if lookup_key("__path__").is_some() {
                        name_str
                    } else {
                        name_str
                            .rsplit_once('.')
                            .map(|(parent, _)| parent.to_string())
                            .unwrap_or_default()
                    }
                } else {
                    String::new()
                }
            }
        };

        if package.is_empty() {
            return Err(PyError::import_error(
                "ImportError",
                "attempted relative import with no known parent package".to_string(),
                None,
            ));
        }

        let parts: Vec<&str> = package.rsplitn(level, '.').collect();
        if parts.len() < level {
            return Err(PyError::import_error(
                "ImportError",
                "attempted relative import beyond top-level package".to_string(),
                None,
            ));
        }
        let base = *parts.last().expect("rsplitn with level >= 1 is non-empty");
        if base.is_empty() {
            return Err(PyError::import_error(
                "ImportError",
                "attempted relative import beyond top-level package".to_string(),
                None,
            ));
        }
        if rel_name.is_empty() {
            Ok(base.to_string())
        } else {
            Ok(format!("{base}.{rel_name}"))
        }
    }

    /// Entry point for VM `ImportModule` instructions (`import ...` and
    /// `from ... import ...`). Resolves parser-encoded leading dots for
    /// relative imports before delegating to [`Self::load_module`].
    pub(crate) fn load_import_module(&mut self, name: &str) -> Result<Value> {
        if name.starts_with('.') {
            let level = name.bytes().take_while(|&b| b == b'.').count();
            let rel_name = &name[level..];
            let resolved = self.resolve_relative_module_name(rel_name, level, None)?;
            return self.load_module(&resolved);
        }
        self.load_module(name)
    }

    /// Check whether `module` exposes a package `__path__` attribute.
    pub(crate) fn module_has_package_path(&mut self, module: &Value) -> Result<bool> {
        if let ValueKind::PyModule(m) = module.kind() {
            if m.borrow().filesystem_namespace().is_some() {
                self.ensure_module_namespace_synced(m);
            }
            let borrowed = m.borrow();
            if let Some(val) = borrowed.get_attr_value("__path__") {
                return Ok(!val.is_unset());
            }
            if borrowed.filesystem_namespace().is_some() || borrowed.live_namespace().is_some() {
                return Ok(false);
            }
        }
        match self.get_attr(module, "__path__") {
            Ok(_) => Ok(true),
            Err(err) if err.class_name_is("AttributeError") => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Extract filesystem search directories from a parent package's
    /// `__path__` attribute. Returns `Ok(None)` when the parent module has no
    /// `__path__` (i.e. is not a package).
    fn extract_package_search_dirs(
        &mut self,
        parent_module: &Value,
    ) -> Result<Option<Vec<PathBuf>>> {
        let path_val = match self.get_attr(parent_module, "__path__") {
            Ok(val) => val,
            Err(err) if err.class_name_is("AttributeError") => return Ok(None),
            Err(err) => return Err(err),
        };
        let dirs = match path_val.kind() {
            ValueKind::List(entries) => entries
                .iter()
                .filter_map(|entry| entry.as_str().map(PathBuf::from))
                .collect(),
            ValueKind::Tuple(entries) => entries
                .iter()
                .filter_map(|entry| entry.as_str().map(PathBuf::from))
                .collect(),
            _ => self
                .collect_iterable(&path_val)?
                .into_iter()
                .filter_map(|entry| entry.as_str().map(PathBuf::from))
                .collect(),
        };
        Ok(Some(dirs))
    }

    /// Return `true` when `err` is a `ModuleNotFoundError` for `expected_name`
    /// itself (not a missing transitive dependency) and `expected_name` is not
    /// explicitly halted via `sys.modules[expected_name] = None`.
    pub(crate) fn is_missing_module_error(&mut self, err: &PyError, expected_name: &str) -> bool {
        let matches_name = match err.kind() {
            pyrust_core::PyErrorKind::ImportError {
                class_name: "ModuleNotFoundError",
                module_name: Some(m),
                ..
            } => m == expected_name,
            pyrust_core::PyErrorKind::Raised(exc) if err.class_name_is("ModuleNotFoundError") => {
                self.get_attr(exc, "name")
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s == expected_name))
                    .unwrap_or(false)
            }
            _ => false,
        };
        if !matches_name {
            return false;
        }
        if self
            .lookup_internal_import_registry(expected_name)
            .is_some_and(|v| v.is_none())
        {
            return false;
        }
        let halted_visible = self
            .lookup_import_registry(expected_name)
            .ok()
            .flatten()
            .is_some_and(|v| v.is_none());
        !halted_visible
    }

    pub(crate) fn load_module(&mut self, name: &str) -> Result<Value> {
        // CPython first probes its interpreter-owned original modules dict. The
        // public `sys.modules` attribute initially aliases it, but rebinding the
        // attribute does not discard already-cached entries. Only a name absent
        // from the original dict reaches importlib's current visible mapping.
        if let Some(cached) =
            validate_import_cache_entry(name, self.lookup_internal_import_registry(name))?
        {
            return Ok(cached);
        }
        if let Some(cached) = self.lookup_sys_modules(name)? {
            return Ok(cached);
        }
        // Present in PyRust's object-owner map but absent from both import
        // registries means the Python-visible cache entry was deleted. Drop the
        // stale owner and load a fresh object.
        let present_internally = self.cold.module_cache.borrow().contains_key(name);
        if present_internally {
            self.cold.module_cache.borrow_mut().remove(name);
        }
        // Built-in modules — declared in
        // `crates/pyrust/src/builtin_modules/mod.rs::pyrust_builtin_modules!`.
        // Adding a new module is a single-line edit there; this file
        // never has to change.
        let builtin = crate::builtin_modules::load_builtin_module(name);
        if let Some(val) = builtin {
            crate::builtin_modules::prepare_builtin_module(name, self, &val);
            self.cold
                .module_cache
                .borrow_mut()
                .insert(name.to_string(), val.clone());
            let initialization = (|| -> Result<()> {
                self.register_in_sys_modules(name, &val)?;
                // Python-source post-load injection: every `@inject` module in
                // `pyrust_builtin_modules!` (collections, asyncio, string,
                // operator, typing, abc, dataclasses, enum, json, …) exec's its
                // `*_py.py` source onto the freshly imported module here.  Done
                // *after* the module is in `module_cache` so each exec (which
                // resolves builtins like `dict`/`tuple`/`property`) sees a
                // consistent import state.  The macro-generated dispatcher keeps
                // the per-module hook list in `mod.rs` — no edit to this site is
                // needed when a new Python-source module is added.
                if let ValueKind::PyModule(m) = val.kind() {
                    crate::builtin_modules::post_load_inject(name, self, m)?;
                }
                // Parent-package identity fix-up: a built-in module like
                // `os` declares `path` as a constant via
                // `super::os_path::module()`, which builds a *fresh*
                // os.path Value rather than the one in `module_cache`.
                // Replace each such submodule-shaped attr with the cached
                // value so `os.path is direct_os_path` matches CPython.
                if let ValueKind::PyModule(m) = val.kind() {
                    let submodule_attrs: Vec<String> = {
                        let snapshot = m.borrow().attrs_snapshot();
                        snapshot
                            .iter()
                            .filter_map(|(attr_name, attr_val)| {
                                // Only consider attrs that are themselves
                                // PyModules — primitive constants stay as-is.
                                match attr_val.kind() {
                                    ValueKind::PyModule(_) => Some(attr_name.clone()),
                                    _ => None,
                                }
                            })
                            .filter(|attr_name| {
                                // And only if there's a registered built-in
                                // by the dotted name — otherwise leave it.
                                let dotted = format!("{name}.{attr_name}");
                                crate::builtin_modules::load_builtin_module(&dotted).is_some()
                            })
                            .collect()
                    };
                    for attr_name in submodule_attrs {
                        let dotted = format!("{name}.{attr_name}");
                        // Recursive load goes through the cache, so the
                        // first such call (whether triggered here or by an
                        // explicit `import os.path`) wins and subsequent
                        // accesses share its identity.
                        let cached_submodule = self.load_module(&dotted)?;
                        m.borrow_mut().insert_attr(attr_name, cached_submodule);
                    }
                }
                // Dotted built-in module parent fix-up: importing a submodule
                // directly (e.g. `importlib.import_module("urllib.parse")` or
                // `import importlib.util as u`) ensures its built-in parent
                // package is also loaded in `sys.modules` and has this
                // submodule bound on it, matching CPython 3.12.
                if let Some((parent, leaf)) = name.rsplit_once('.')
                    && crate::builtin_modules::load_builtin_module(parent).is_some()
                {
                    let parent_mod = if let Some(cached) = validate_import_cache_entry(
                        parent,
                        self.lookup_internal_import_registry(parent),
                    )? {
                        cached
                    } else if let Some(cached) = self.lookup_sys_modules(parent)? {
                        cached
                    } else {
                        self.load_module(parent)?
                    };
                    let _ = self.assign_attr(parent_mod, leaf, val.clone());
                }
                Ok(())
            })();
            if let Err(error) = initialization {
                self.unregister_failed_module(name);
                return Err(error);
            }
            return Ok(val);
        }

        // User filesystem packages (`<pkg>/__init__.py`) and modules (`<mod>.py`).
        // Dotted names load parent packages first and search each child inside
        // its parent package's `__path__` (never falling back to `sys.path`).
        let (parent_name, leaf_name) = match name.rsplit_once('.') {
            Some((parent, leaf)) if !parent.is_empty() && !leaf.is_empty() => (Some(parent), leaf),
            _ => (None, name),
        };
        let (parent_module_value, search_dirs) = if let Some(parent) = parent_name {
            let loaded_parent = if let Some(cached) =
                validate_import_cache_entry(parent, self.lookup_internal_import_registry(parent))?
            {
                cached
            } else if let Some(cached) = self.lookup_sys_modules(parent)? {
                cached
            } else {
                self.load_module(parent)?
            };
            // Executing `parent/__init__.py` may have imported `name` as a side
            // effect; re-probe `sys.modules` before searching the filesystem.
            if let Some(cached) =
                validate_import_cache_entry(name, self.lookup_internal_import_registry(name))?
            {
                return Ok(cached);
            }
            if let Some(cached) = self.lookup_sys_modules(name)? {
                return Ok(cached);
            }
            // Re-read the active parent from `sys.modules` in case the parent's
            // `__init__.py` replaced its own entry.
            let active_parent =
                validate_import_cache_entry(parent, self.lookup_internal_import_registry(parent))?
                    .or(self.lookup_sys_modules(parent)?)
                    .unwrap_or(loaded_parent);
            let Some(dirs) = self.extract_package_search_dirs(&active_parent)? else {
                return Err(PyError::import_error(
                    "ModuleNotFoundError",
                    format!("No module named '{name}'; '{parent}' is not a package"),
                    Some(name.to_string()),
                ));
            };
            (Some(active_parent), dirs)
        } else {
            (None, self.user_module_search_dirs())
        };

        if let Some(target) = find_filesystem_module(&search_dirs, leaf_name) {
            let full_path = target.file_path;
            let src = std::fs::read_to_string(&full_path).map_err(|e| {
                PyError::runtime(format!("failed to read '{}': {e}", full_path.display()))
            })?;
            // The module's own path: tags its code objects' `co_filename` and
            // the traceback `File "..."` line for frames raised inside it,
            // instead of inheriting the importing script's path (#2438).
            let module_filename = full_path.to_string_lossy().into_owned();
            // Parse with per-statement line numbers and per-token positions so
            // frames raised inside the imported module carry the correct
            // `tb_lineno` and PEP 657 caret anchors (#2438); the import path
            // previously parsed without line info, so module frames printed no
            // line number / source line.
            let (program, linenos) = Self::parse_source_to_stmts_with_linenos(&src)?;
            // Subinterpreter shares the same module_cache so results are visible to parent
            let mut sub = Interpreter {
                // Filesystem imports execute through a child object but
                // remain inside the parent's Python interpreter. Sharing
                // warnings state mirrors the singleton `warnings` module
                // already shared through `module_cache`/`sys.modules`.
                warnings_state: Rc::clone(&self.warnings_state),
                sys_state: Rc::clone(&self.sys_state),
                vm_hooks_flags: self.vm_hooks_flags,
                // This child is an implementation detail of importing a
                // module into the same Python interpreter. Start with the
                // parent's integer conversion policy; changes are copied
                // back below even if module execution raises.
                int_max_str_digits: get_int_max_str_digits(self),
                // Imported Python modules execute in a child Interpreter
                // but share the parent's recursion policy.
                recursion_limit: self.recursion_limit,
                cold: Box::new(InterpreterColdState {
                    script_dir: self.cold.script_dir.clone(),
                    script_filename: Some(std::sync::Arc::from(module_filename.as_str())),
                    // Imported modules run in the top-level script's process
                    // context, so a later `import sys` must see the original
                    // command-line arguments rather than an empty vector.
                    script_argv: self.cold.script_argv.clone(),
                    module_cache: Rc::clone(&self.cold.module_cache),
                    bootstrap_module_registry: self.cold.bootstrap_module_registry.clone(),
                    atexit_state: Rc::clone(&self.cold.atexit_state),
                    ..Default::default()
                }),
                ..Default::default()
            };
            // CPython 3.12 `__package__` rules:
            // - Packages (`__init__.py`): equal to the full package name.
            // - Submodules: equal to the parent package name.
            // - Top-level modules: empty string `""`.
            let package_name = if target.package_dir.is_some() {
                name
            } else {
                parent_name.unwrap_or("")
            };
            // Source modules and functions must observe one namespace
            // owner, not a post-execution `env.values -> PyModule.attrs`
            // snapshot. The module keeps the root globals dict strongly
            // and only a weak environment link; captured functions keep
            // their own root alive, while circular imports do not gain a
            // direct strong PyModule/Environment ownership cycle.
            let module_rc = Rc::new(RefCell::new(PyModule::new(
                name.to_string(),
                crate::value::ModuleAttrs::default(),
            )));
            {
                let mut module = module_rc.borrow_mut();
                module.attach_filesystem_namespace(&sub.env);
                // Seed import metadata in CPython 3.12's exact key order
                // (`__name__`, `__doc__`, `__package__`, `__loader__`,
                // `__spec__`, [`__path__`], `__file__`) before registering in
                // `sys.modules` and before executing the body so circular
                // imports and child imports inside `__init__.py` immediately
                // observe `__package__` and `__path__`. The script dunder
                // initializer uses get-or-insert and appends only the
                // remaining slots (`__cached__`, `__builtins__`, etc.).
                module.insert_attr("__name__".to_string(), Value::string(name));
                module.insert_attr("__doc__".to_string(), Value::none());
                module.insert_attr("__package__".to_string(), Value::string(package_name));
                module.insert_attr("__loader__".to_string(), Value::none());
                module.insert_attr("__spec__".to_string(), Value::none());
                if let Some(ref pkg_dir) = target.package_dir {
                    module.insert_attr(
                        "__path__".to_string(),
                        Value::list(vec![Value::string(pkg_dir.to_string_lossy())]),
                    );
                }
                module.insert_attr("__file__".to_string(), Value::string(&module_filename));
            }
            let module = Value::py_module(Rc::clone(&module_rc));
            // Register before executing the body so a circular import sees
            // this partial object. Script writes are mirrored into its live
            // backing immediately, making already-executed bindings visible.
            self.cold
                .module_cache
                .borrow_mut()
                .insert(name.to_string(), module.clone());
            if let Err(error) = self.register_in_sys_modules(name, &module) {
                self.unregister_failed_module(name);
                return Err(error);
            }
            // call_depth is thread_local — the child automatically shares
            // the active host-stack counter.
            let execution_result = sub.exec_import_program_with_linenos(&program, &linenos, &src);
            let child_int_max_str_digits = get_int_max_str_digits(&sub);
            set_int_max_str_digits(self, child_int_max_str_digits);
            if let Err(e) = execution_result {
                // CPython removes a half-initialised module from sys.modules
                // (and thus the import cache) when its body raises, so a
                // later import retries from scratch instead of yielding a
                // partially-populated object.
                self.unregister_failed_module(name);
                return Err(e);
            }
            let final_module = self
                .lookup_internal_import_registry(name)
                .or_else(|| self.lookup_sys_modules(name).ok().flatten())
                .unwrap_or(module);
            if let Some(parent) = parent_name {
                let current_parent = self
                    .lookup_internal_import_registry(parent)
                    .or_else(|| self.lookup_sys_modules(parent).ok().flatten())
                    .or(parent_module_value);
                if let Some(parent_mod) = current_parent {
                    let _ = self.assign_attr(parent_mod, leaf_name, final_module.clone());
                }
            }
            return Ok(final_module);
        }

        Err(PyError::import_error(
            "ModuleNotFoundError",
            format!("No module named '{name}'"),
            Some(name.to_string()),
        ))
    }

    /// Resolve a `-m <module_name>` target for execution as `__main__`,
    /// importing any parent package (and the package itself when running
    /// `<pkg>/__main__.py`) into `sys.modules` so relative imports work.
    pub(crate) fn resolve_module_for_main(
        &mut self,
        module_name: &str,
    ) -> std::result::Result<MainModuleTarget, MainModuleResolveError> {
        if module_name.starts_with('.') {
            return Err(MainModuleResolveError::Message(
                "Relative module names not supported".to_string(),
            ));
        }
        if module_name.is_empty() || module_name.contains("..") {
            return Err(MainModuleResolveError::Message(format!(
                "No module named {module_name}"
            )));
        }
        if crate::builtin_modules::load_builtin_module(module_name).is_some() {
            return Err(MainModuleResolveError::Message(format!(
                "No code object available for {module_name}"
            )));
        }

        let load_pkg_or_map_err = |interp: &mut Self,
                                   pkg_name: &str|
         -> std::result::Result<Value, MainModuleResolveError> {
            pyrust_core::reset_captured_error_frames();
            match interp.load_module(pkg_name) {
                Ok(val) => Ok(val),
                Err(err) => {
                    let frames = pyrust_core::take_captured_error_frames().unwrap_or_default();
                    if frames.is_empty()
                        && let pyrust_core::PyErrorKind::ImportError {
                            class_name: "ModuleNotFoundError",
                            ref message,
                            ..
                        } = *err.kind()
                    {
                        return Err(MainModuleResolveError::Message(message.clone()));
                    }
                    Err(MainModuleResolveError::ImportException(err, frames))
                }
            }
        };

        let resolve_package_main = |interp: &mut Self,
                                    pkg_name: &str|
         -> std::result::Result<
            MainModuleTarget,
            MainModuleResolveError,
        > {
            let pkg_mod = load_pkg_or_map_err(interp, pkg_name)?;
            let pkg_dirs = interp
                .extract_package_search_dirs(&pkg_mod)
                .map_err(|err| {
                    let frames = pyrust_core::take_captured_error_frames().unwrap_or_default();
                    MainModuleResolveError::ImportException(err, frames)
                })?
                .ok_or_else(|| {
                    MainModuleResolveError::Message(format!(
                        "No module named {pkg_name}.__main__; '{pkg_name}' is not a package"
                    ))
                })?;
            match find_filesystem_module(&pkg_dirs, "__main__") {
                Some(main_target) if main_target.package_dir.is_none() => Ok(MainModuleTarget {
                    file_path: main_target.file_path,
                    package_name: pkg_name.to_string(),
                }),
                Some(_) => Err(MainModuleResolveError::Message(format!(
                    "Cannot execute package '{pkg_name}.__main__'"
                ))),
                None => Err(MainModuleResolveError::Message(format!(
                    "No module named {pkg_name}.__main__; '{pkg_name}' is a package and cannot be directly executed"
                ))),
            }
        };

        if let Some((parent_name, leaf_name)) = module_name.rsplit_once('.') {
            let parent_mod = load_pkg_or_map_err(self, parent_name)?;
            if leaf_name.is_empty() {
                return Err(MainModuleResolveError::Message(format!(
                    "No module named {module_name}"
                )));
            }
            let Some(parent_dirs) =
                self.extract_package_search_dirs(&parent_mod)
                    .map_err(|err| {
                        let frames = pyrust_core::take_captured_error_frames().unwrap_or_default();
                        MainModuleResolveError::ImportException(err, frames)
                    })?
            else {
                return Err(MainModuleResolveError::Message(format!(
                    "No module named '{module_name}'; '{parent_name}' is not a package"
                )));
            };
            let Some(target) = find_filesystem_module(&parent_dirs, leaf_name) else {
                return Err(MainModuleResolveError::Message(format!(
                    "No module named {module_name}"
                )));
            };
            if target.package_dir.is_some() {
                resolve_package_main(self, module_name)
            } else {
                Ok(MainModuleTarget {
                    file_path: target.file_path,
                    package_name: parent_name.to_string(),
                })
            }
        } else {
            let search_dirs = self.user_module_search_dirs();
            let Some(target) = find_filesystem_module(&search_dirs, module_name) else {
                return Err(MainModuleResolveError::Message(format!(
                    "No module named {module_name}"
                )));
            };
            if target.package_dir.is_some() {
                resolve_package_main(self, module_name)
            } else {
                Ok(MainModuleTarget {
                    file_path: target.file_path,
                    package_name: String::new(),
                })
            }
        }
    }
}
