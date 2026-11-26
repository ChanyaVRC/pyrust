struct FilesystemModuleTarget {
    file_path: PathBuf,
    package_dir: Option<PathBuf>,
}

/// Resolved execution target for `pyrust -m <module>`.
pub(crate) enum MainModuleTarget {
    File {
        file_path: PathBuf,
        package_name: String,
    },
    Builtin {
        virtual_path: String,
        source: &'static str,
    },
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

/// Result of looking up a single `sys.path` / `__path__` entry in
/// `sys.path_importer_cache` and `sys.path_hooks`.
enum PathEntryFinderLookup {
    /// A finder returned by a `sys.path_hooks` callable or cached in
    /// `sys.path_importer_cache`.
    Finder(Value),
    /// Entry explicitly mapped to `None` in `sys.path_importer_cache` (or
    /// rejected by all `sys.path_hooks` and not a filesystem directory).
    CachedNone,
    /// No hook claimed this entry; fall back to default filesystem lookup.
    DefaultFilesystem,
}

/// Locate a filesystem package (`<dir>/<leaf>/__init__.py`) or module
/// (`<dir>/<leaf>.py`) within a single search directory.
///
/// Within a directory, a package takes precedence over a sibling `.py` module,
/// matching CPython 3.12's `FileFinder`.
fn find_filesystem_module_in_dir(
    dir: &std::path::Path,
    leaf_name: &str,
) -> Option<FilesystemModuleTarget> {
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
    None
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
        if let Some(target) = find_filesystem_module_in_dir(dir, leaf_name) {
            return Some(target);
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

    /// Populate the per-interpreter `sys` module attributes when `sys` is first
    /// imported. Applies `RuntimeConfig` flags, `warnoptions`, `_xoptions`,
    /// `dont_write_bytecode`, `pycache_prefix`, `platlibdir`, prefixes,
    /// `orig_argv`, and `PYTHONPATH`.
    pub(crate) fn initialize_system_module(&self, module: &Value) {
        let ValueKind::PyModule(sys) = module.kind() else {
            return;
        };
        let cfg = &self.cold.runtime_config;
        let mut initial_paths: Vec<Value> = Vec::new();
        if !self.cold.cli_startup_managed && !cfg.safe_path {
            let initial_path = self
                .cold
                .script_dir
                .as_ref()
                .map(|dir| Value::string(dir.to_string_lossy()))
                .unwrap_or_else(|| Value::string(""));
            initial_paths.push(initial_path);
        }
        for entry in &cfg.python_path {
            initial_paths.push(Value::string(entry));
        }

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
        sys.insert_attr("path".to_string(), Value::list(initial_paths));
        sys.insert_attr(
            "modules".to_string(),
            self.cold.bootstrap_module_registry.clone(),
        );
        // Apply CLI + `PYTHON*` runtime configuration (#3243).
        // (`sys.flags` is finalized in `builtin_modules/finalization.rs` so
        // runtime code stays decoupled from concrete built-in module bodies.)
        sys.insert_attr(
            "dont_write_bytecode".to_string(),
            Value::bool_(cfg.dont_write_bytecode != 0),
        );
        sys.insert_attr(
            "pycache_prefix".to_string(),
            cfg.pycache_prefix
                .as_deref()
                .map(Value::string)
                .unwrap_or_else(Value::none),
        );
        if let Some(ref platlibdir) = cfg.platlibdir {
            sys.insert_attr("platlibdir".to_string(), Value::string(platlibdir));
        }
        if let Some(ref prefix) = cfg.prefix_override {
            sys.insert_attr("prefix".to_string(), Value::string(prefix));
            sys.insert_attr("base_prefix".to_string(), Value::string(prefix));
            let exec_prefix = cfg.exec_prefix_override.as_deref().unwrap_or(prefix);
            sys.insert_attr("exec_prefix".to_string(), Value::string(exec_prefix));
            sys.insert_attr("base_exec_prefix".to_string(), Value::string(exec_prefix));
        } else if let Some(ref venv_prefix) = cfg.venv_prefix {
            sys.insert_attr("prefix".to_string(), Value::string(venv_prefix));
            sys.insert_attr("exec_prefix".to_string(), Value::string(venv_prefix));
            if let Some(ref base_prefix) = cfg.venv_base_prefix {
                sys.insert_attr("base_prefix".to_string(), Value::string(base_prefix));
                sys.insert_attr("base_exec_prefix".to_string(), Value::string(base_prefix));
            }
            if let Some(ref venv_exe) = cfg.venv_executable {
                sys.insert_attr("executable".to_string(), Value::string(venv_exe));
            }
            if let Some(ref base_exe) = cfg.venv_base_executable {
                sys.insert_attr("_base_executable".to_string(), Value::string(base_exe));
            }
            if let Some(ref home) = cfg.venv_home {
                sys.insert_attr("_home".to_string(), Value::string(home));
            }
        }
        // Fresh per-interpreter mutable sys containers and aliases (#2970).
        let orig_argv_src = if !cfg.orig_argv.is_empty() {
            &cfg.orig_argv
        } else {
            &self.cold.script_argv
        };
        let orig_argv: Vec<Value> = orig_argv_src.iter().map(Value::string).collect();
        sys.insert_attr("orig_argv".to_string(), Value::list(orig_argv));
        let warnoptions: Vec<Value> = cfg.warnoptions.iter().map(Value::string).collect();
        sys.insert_attr("warnoptions".to_string(), Value::list(warnoptions));
        sys.insert_attr("meta_path".to_string(), Value::list(Vec::new()));
        sys.insert_attr("path_hooks".to_string(), Value::list(Vec::new()));
        sys.insert_attr(
            "path_importer_cache".to_string(),
            Value::dict(PyDict::default()),
        );
        let mut xopts = PyDict::default();
        for (k, v) in &cfg.xoptions {
            let py_val = match v {
                Some(s) => Value::string(s),
                None => Value::bool_(true),
            };
            xopts.insert(PyKey::str_from(k), py_val);
        }
        sys.insert_attr("_xoptions".to_string(), Value::dict(xopts));
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

    /// Return an attribute from the already-cached `sys` module, if `sys` has
    /// been imported into this interpreter. Never triggers an import of `sys`.
    fn cached_sys_attr(&self, attr: &str) -> Option<Value> {
        self.cold
            .module_cache
            .borrow()
            .get("sys")
            .and_then(|module| {
                let ValueKind::PyModule(sys) = module.kind() else {
                    return None;
                };
                sys.borrow().get_attr_value(attr)
            })
    }

    /// Snapshot the import roots currently exposed through `sys.path`.
    /// Before `sys` is imported, preserve the legacy script-directory lookup
    /// so a script can still import a sibling module as its very first import.
    fn user_module_search_dirs(&self) -> Vec<PathBuf> {
        if let Some(path) = self.cached_sys_attr("path")
            && let ValueKind::List(entries) = path.kind()
        {
            return entries
                .iter()
                .filter_map(|entry| entry.as_str().map(PathBuf::from))
                .collect();
        }

        self.cold.script_dir.iter().cloned().collect()
    }

    /// Collect raw search entries from `sys.path` (or `script_dir` before `sys`
    /// is loaded) so `sys.path_hooks` can see non-filesystem path tokens as
    /// well as `os.PathLike` values.
    fn user_module_search_entries(&mut self) -> Result<Vec<Value>> {
        if let Some(path) = self.cached_sys_attr("path") {
            return self.collect_iterable(&path);
        }
        Ok(self
            .cold
            .script_dir
            .iter()
            .map(|dir| Value::string(dir.to_string_lossy()))
            .collect())
    }

    /// Convert a `sys.path` / `__path__` entry into a string path using `str`
    /// or `os.PathLike.__fspath__`. Returns `Ok(None)` for non-path entries.
    fn coerce_search_path_entry(&mut self, entry: &Value) -> Result<Option<String>> {
        if let Some(s) = entry.as_str() {
            return Ok(Some(s.to_string()));
        }
        if let Ok(fspath) = self.get_attr(entry, "__fspath__") {
            let res = self.call_function_expanded(fspath, &[])?;
            if let Some(s) = res.as_str() {
                return Ok(Some(s.to_string()));
            }
        }
        Ok(None)
    }

    /// Snapshot `sys.meta_path` finders when `sys` is loaded and the list is
    /// non-empty. Returns an empty `Vec` on the fast path when no custom meta
    /// path finders are installed.
    fn snapshot_meta_path_finders(&mut self) -> Result<Vec<Value>> {
        let Some(meta_path) = self.cached_sys_attr("meta_path") else {
            return Ok(Vec::new());
        };
        if meta_path.is_none() {
            return Ok(Vec::new());
        }
        if let ValueKind::List(items) = meta_path.kind() {
            return Ok(items.clone());
        }
        self.collect_iterable(&meta_path)
    }

    /// Consult `sys.path_importer_cache` and `sys.path_hooks` for a single
    /// search-path entry.
    fn lookup_path_hook_finder(&mut self, entry_str: &str) -> Result<PathEntryFinderLookup> {
        let Some(cache) = self.cached_sys_attr("path_importer_cache") else {
            return Ok(PathEntryFinderLookup::DefaultFilesystem);
        };
        let hooks_val = self.cached_sys_attr("path_hooks");
        let cache_key = if entry_str.is_empty() {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            entry_str.to_string()
        };

        // Probe `sys.path_importer_cache` first.
        let cache_is_dict = matches!(cache.kind(), ValueKind::Dict(_));
        let cached_entry: Option<Value> = if cache_is_dict {
            cache
                .dict_with(|d| d.get(&PyKey::str_from(&cache_key)).cloned())
                .flatten()
        } else {
            match self.get_attr(&cache, "__getitem__") {
                Ok(getitem) => match self.call_function_expanded(
                    getitem,
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string(&cache_key),
                    }],
                ) {
                    Ok(v) => Some(v),
                    Err(e) if e.class_name_is("KeyError") => None,
                    Err(e) => return Err(e),
                },
                Err(_) => None,
            }
        };

        if let Some(finder) = cached_entry {
            if finder.is_none() {
                return Ok(PathEntryFinderLookup::CachedNone);
            }
            return Ok(PathEntryFinderLookup::Finder(finder));
        }

        // Not cached yet — consult `sys.path_hooks` if any hooks are registered.
        let hooks: Vec<Value> = match hooks_val {
            Some(ref h) if !h.is_none() => match h.kind() {
                ValueKind::List(items) => items.clone(),
                _ => self.collect_iterable(h)?,
            },
            _ => return Ok(PathEntryFinderLookup::DefaultFilesystem),
        };
        if hooks.is_empty() {
            return Ok(PathEntryFinderLookup::DefaultFilesystem);
        }

        let store_in_cache =
            |interp: &mut Self, cache_obj: &Value, key: &str, val: Value| -> Result<()> {
                let is_dict = matches!(cache_obj.kind(), ValueKind::Dict(_));
                if is_dict {
                    cache_obj.dict_insert(PyKey::str_from(key), val)?;
                } else if let Ok(setitem) = interp.get_attr(cache_obj, "__setitem__") {
                    interp.call_function_expanded(
                        setitem,
                        &[
                            ExpandedCallArg {
                                name: None,
                                value: Value::string(key),
                            },
                            ExpandedCallArg {
                                name: None,
                                value: val,
                            },
                        ],
                    )?;
                }
                Ok(())
            };

        for hook in hooks {
            match self.call_function_expanded(
                hook,
                &[ExpandedCallArg {
                    name: None,
                    value: Value::string(&cache_key),
                }],
            ) {
                Ok(finder) => {
                    store_in_cache(self, &cache, &cache_key, finder.clone())?;
                    if finder.is_none() {
                        return Ok(PathEntryFinderLookup::CachedNone);
                    }
                    return Ok(PathEntryFinderLookup::Finder(finder));
                }
                Err(e)
                    if e.class_name_is("ImportError") || e.class_name_is("ModuleNotFoundError") =>
                {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        // All hooks rejected this entry with `ImportError`. Cache `None` for
        // non-directory entries (matching CPython 3.12's `PathFinder` cache
        // behavior) while allowing real filesystem directories to fall through
        // to PyRust's built-in filesystem loader.
        let path_probe = if cache_key.is_empty() {
            std::path::Path::new(".")
        } else {
            std::path::Path::new(&cache_key)
        };
        if !path_probe.is_dir() {
            store_in_cache(self, &cache, &cache_key, Value::none())?;
            return Ok(PathEntryFinderLookup::CachedNone);
        }
        Ok(PathEntryFinderLookup::DefaultFilesystem)
    }

    /// Create, register, and execute a module from a Python `ModuleSpec`
    /// returned by a `sys.meta_path` or `sys.path_hooks` finder.
    fn load_module_from_spec(
        &mut self,
        name: &str,
        spec: Value,
        parent_name: Option<&str>,
        parent_module_value: Option<Value>,
    ) -> Result<Value> {
        let loader = match self.get_attr(&spec, "loader") {
            Ok(v) => v,
            Err(e) if e.class_name_is("AttributeError") => Value::none(),
            Err(e) => return Err(e),
        };

        let mut created: Value = Value::none();
        if !loader.is_none() {
            match self.get_attr(&loader, "create_module") {
                Ok(create_fn) => {
                    created = self.call_function_expanded(
                        create_fn,
                        &[ExpandedCallArg {
                            name: None,
                            value: spec.clone(),
                        }],
                    )?;
                }
                Err(e) if e.class_name_is("AttributeError") => {
                    if self.get_attr(&loader, "exec_module").is_ok() {
                        return Err(PyError::import_error(
                            "ImportError",
                            "loaders that define exec_module() must also define create_module()"
                                .to_string(),
                            Some(name.to_string()),
                        ));
                    }
                }
                Err(e) => return Err(e),
            }
        }

        let spec_name = match self.get_attr(&spec, "name") {
            Ok(v) if v.as_str().is_some() => v.as_str().unwrap_or(name).to_string(),
            _ => name.to_string(),
        };

        let module = if created.is_none() {
            let module_rc = Rc::new(RefCell::new(PyModule::new(
                spec_name.clone(),
                crate::value::ModuleAttrs::default(),
            )));
            {
                let mut m = module_rc.borrow_mut();
                m.attach_live_namespace();
                m.insert_attr("__name__".to_string(), Value::string(&spec_name));
                m.insert_attr("__doc__".to_string(), Value::none());
                m.insert_attr("__package__".to_string(), Value::none());
                m.insert_attr("__loader__".to_string(), Value::none());
                m.insert_attr("__spec__".to_string(), Value::none());
            }
            Value::py_module(module_rc)
        } else {
            created
        };

        // Initialize standard module attributes from `spec`, matching CPython's
        // `_init_module_attrs(spec, module, override=False)`.
        let attr_missing_or_none = |interp: &mut Self, obj: &Value, attr: &str| -> bool {
            match interp.get_attr(obj, attr) {
                Ok(v) => v.is_none() || v.is_unset(),
                Err(_) => true,
            }
        };

        if attr_missing_or_none(self, &module, "__name__") {
            let _ = self.assign_attr(module.clone(), "__name__", Value::string(&spec_name));
        }

        let smsl = match self.get_attr(&spec, "submodule_search_locations") {
            Ok(v) => v,
            Err(_) => Value::none(),
        };

        let mut effective_loader = loader.clone();
        if attr_missing_or_none(self, &module, "__loader__") {
            if effective_loader.is_none()
                && !smsl.is_none()
                && let Ok(machinery) = self.load_module("importlib.machinery")
                && let Ok(ns_loader_cls) = self.get_attr(&machinery, "NamespaceLoader")
                && let Ok(ns_loader) = self.call_function_expanded(
                    ns_loader_cls,
                    &[
                        ExpandedCallArg {
                            name: None,
                            value: Value::string(&spec_name),
                        },
                        ExpandedCallArg {
                            name: None,
                            value: smsl.clone(),
                        },
                        ExpandedCallArg {
                            name: None,
                            value: Value::none(),
                        },
                    ],
                )
            {
                effective_loader = ns_loader.clone();
                let _ = self.assign_attr(spec.clone(), "loader", ns_loader);
                let _ = self.assign_attr(module.clone(), "__file__", Value::none());
            }
            let _ = self.assign_attr(module.clone(), "__loader__", effective_loader.clone());
        }

        if attr_missing_or_none(self, &module, "__package__") {
            let pkg_val = match self.get_attr(&spec, "parent") {
                Ok(v) if !v.is_none() => v,
                _ => {
                    if !smsl.is_none() {
                        Value::string(&spec_name)
                    } else {
                        let parent_str = spec_name.rsplit_once('.').map(|(p, _)| p).unwrap_or("");
                        Value::string(parent_str)
                    }
                }
            };
            let _ = self.assign_attr(module.clone(), "__package__", pkg_val);
        }

        let _ = self.assign_attr(module.clone(), "__spec__", spec.clone());

        if !smsl.is_none() && attr_missing_or_none(self, &module, "__path__") {
            let _ = self.assign_attr(module.clone(), "__path__", smsl);
        }

        let has_location = match self.get_attr(&spec, "has_location") {
            Ok(v) => self.truthy_value(&v).unwrap_or(false),
            Err(_) => false,
        };
        if has_location {
            if attr_missing_or_none(self, &module, "__file__")
                && let Ok(origin) = self.get_attr(&spec, "origin")
            {
                let _ = self.assign_attr(module.clone(), "__file__", origin);
            }
            if attr_missing_or_none(self, &module, "__cached__")
                && let Ok(cached) = self.get_attr(&spec, "cached")
            {
                let _ = self.assign_attr(module.clone(), "__cached__", cached);
            }
        }

        // Register in `sys.modules` BEFORE executing the module body so
        // circular imports and `sys.modules[__name__]` lookups succeed.
        self.cold
            .module_cache
            .borrow_mut()
            .insert(name.to_string(), module.clone());
        if let Err(error) = self.register_in_sys_modules(name, &module) {
            self.unregister_failed_module(name);
            return Err(error);
        }

        if !effective_loader.is_none()
            && let Ok(exec_fn) = self.get_attr(&effective_loader, "exec_module")
            && let Err(e) = self.call_function_expanded(
                exec_fn,
                &[ExpandedCallArg {
                    name: None,
                    value: module.clone(),
                }],
            )
        {
            self.unregister_failed_module(name);
            return Err(e);
        }

        let final_module = self
            .lookup_internal_import_registry(name)
            .or_else(|| self.lookup_sys_modules(name).ok().flatten())
            .unwrap_or(module);

        if let Some(parent) = parent_name {
            let leaf_name = name.rsplit_once('.').map(|(_, l)| l).unwrap_or(name);
            let current_parent = self
                .lookup_internal_import_registry(parent)
                .or_else(|| self.lookup_sys_modules(parent).ok().flatten())
                .or(parent_module_value);
            if let Some(parent_mod) = current_parent {
                let _ = self.assign_attr(parent_mod, leaf_name, final_module.clone());
            }
        }

        Ok(final_module)
    }

    /// Construct and register a PEP 420 implicit namespace package spanning
    /// `namespace_dirs`.
    fn create_namespace_package(
        &mut self,
        name: &str,
        namespace_dirs: Vec<String>,
        parent_name: Option<&str>,
        parent_module_value: Option<Value>,
    ) -> Result<Value> {
        let path_list = Value::list(namespace_dirs.into_iter().map(Value::string).collect());

        let (loader_val, spec_val) = if let Ok(machinery) = self.load_module("importlib.machinery")
            && let Ok(ns_loader_cls) = self.get_attr(&machinery, "NamespaceLoader")
            && let Ok(spec_cls) = self.get_attr(&machinery, "ModuleSpec")
        {
            let path_finder = self
                .get_attr(&machinery, "PathFinder")
                .unwrap_or_else(|_| Value::none());
            let loader = self
                .call_function_expanded(
                    ns_loader_cls,
                    &[
                        ExpandedCallArg {
                            name: None,
                            value: Value::string(name),
                        },
                        ExpandedCallArg {
                            name: None,
                            value: path_list.clone(),
                        },
                        ExpandedCallArg {
                            name: None,
                            value: path_finder,
                        },
                    ],
                )
                .unwrap_or_else(|_| Value::none());
            let spec = self
                .call_function_expanded(
                    spec_cls,
                    &[
                        ExpandedCallArg {
                            name: None,
                            value: Value::string(name),
                        },
                        ExpandedCallArg {
                            name: None,
                            value: loader.clone(),
                        },
                        ExpandedCallArg {
                            name: Some("origin".to_string()),
                            value: Value::none(),
                        },
                    ],
                )
                .unwrap_or_else(|_| Value::none());
            if !spec.is_none() {
                let _ = self.assign_attr(
                    spec.clone(),
                    "submodule_search_locations",
                    path_list.clone(),
                );
            }
            (loader, spec)
        } else {
            (Value::none(), Value::none())
        };

        let module_rc = Rc::new(RefCell::new(PyModule::new(
            name.to_string(),
            crate::value::ModuleAttrs::default(),
        )));
        {
            let mut module = module_rc.borrow_mut();
            module.attach_live_namespace();
            module.insert_attr("__name__".to_string(), Value::string(name));
            module.insert_attr("__doc__".to_string(), Value::none());
            module.insert_attr("__package__".to_string(), Value::string(name));
            module.insert_attr("__loader__".to_string(), loader_val);
            module.insert_attr("__spec__".to_string(), spec_val);
            module.insert_attr("__path__".to_string(), path_list);
            module.insert_attr("__file__".to_string(), Value::none());
        }
        let module = Value::py_module(module_rc);
        self.cold
            .module_cache
            .borrow_mut()
            .insert(name.to_string(), module.clone());
        if let Err(error) = self.register_in_sys_modules(name, &module) {
            self.unregister_failed_module(name);
            return Err(error);
        }
        if let Some(parent) = parent_name {
            let leaf_name = name.rsplit_once('.').map(|(_, l)| l).unwrap_or(name);
            let current_parent = self
                .lookup_internal_import_registry(parent)
                .or_else(|| self.lookup_sys_modules(parent).ok().flatten())
                .or(parent_module_value);
            if let Some(parent_mod) = current_parent {
                let _ = self.assign_attr(parent_mod, leaf_name, module.clone());
            }
        }
        Ok(module)
    }

    /// Notify all registered `sys.addaudithook` callbacks of an audit event.
    pub(crate) fn emit_audit_event(&mut self, event: &str, event_args: &[Value]) -> Result<()> {
        let hooks: Vec<Value> = self.features().sys_with(|state| state.audit_hooks.clone());
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

    /// Check whether `module` exposes a package `__path__` attribute, or is a
    /// built-in package registered in `pyrust_builtin_modules!`.
    pub(crate) fn module_has_package_path(&mut self, module: &Value) -> Result<bool> {
        if let ValueKind::PyModule(m) = module.kind() {
            if m.borrow().filesystem_namespace().is_some() {
                self.ensure_module_namespace_synced(m);
            }
            let borrowed = m.borrow();
            if let Some(val) = borrowed.get_attr_value("__path__") {
                return Ok(!val.is_unset());
            }
            if borrowed.filesystem_namespace().is_some() {
                return Ok(false);
            }
            if crate::builtin_modules::is_builtin_package(&borrowed.name) {
                return Ok(true);
            }
            if borrowed.live_namespace().is_some() {
                return Ok(false);
            }
        }
        match self.get_attr(module, "__path__") {
            Ok(_) => Ok(true),
            Err(err) if err.class_name_is("AttributeError") => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// Extract raw search entries from a parent package's `__path__` attribute.
    /// Returns `Ok(None)` when the parent module has no `__path__`.
    fn extract_package_search_entries(
        &mut self,
        parent_module: &Value,
    ) -> Result<Option<Vec<Value>>> {
        let path_val = match self.get_attr(parent_module, "__path__") {
            Ok(val) => val,
            Err(err) if err.class_name_is("AttributeError") => return Ok(None),
            Err(err) => return Err(err),
        };
        let entries = match path_val.kind() {
            ValueKind::List(entries) => entries.clone(),
            ValueKind::Tuple(entries) => entries.to_vec(),
            _ => self.collect_iterable(&path_val)?,
        };
        Ok(Some(entries))
    }

    /// Extract filesystem search directories from a parent package's
    /// `__path__` attribute. Returns `Ok(None)` when the parent module has no
    /// `__path__` (i.e. is not a package).
    fn extract_package_search_dirs(
        &mut self,
        parent_module: &Value,
    ) -> Result<Option<Vec<PathBuf>>> {
        let Some(entries) = self.extract_package_search_entries(parent_module)? else {
            return Ok(None);
        };
        let dirs = entries
            .iter()
            .filter_map(|entry| entry.as_str().map(PathBuf::from))
            .collect();
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
        // Split dotted module names into `(parent, leaf)` up front so custom
        // `sys.meta_path` finders and path-based loaders share parent loading.
        let (parent_name, leaf_name) = match name.rsplit_once('.') {
            Some((parent, leaf)) if !parent.is_empty() && !leaf.is_empty() => (Some(parent), leaf),
            _ => (None, name),
        };

        // 1. Consult user-installed `sys.meta_path` finders BEFORE falling back
        // to built-in or default filesystem lookup (PEP 451 / CPython 3.12).
        let meta_path_finders = self.snapshot_meta_path_finders()?;
        if !meta_path_finders.is_empty() {
            let (meta_parent_mod, meta_parent_path) = if let Some(parent) = parent_name {
                let loaded_parent = if let Some(cached) = validate_import_cache_entry(
                    parent,
                    self.lookup_internal_import_registry(parent),
                )? {
                    cached
                } else if let Some(cached) = self.lookup_sys_modules(parent)? {
                    cached
                } else {
                    self.load_module(parent)?
                };
                // Importing the parent may have populated `name` in `sys.modules`.
                if let Some(cached) =
                    validate_import_cache_entry(name, self.lookup_internal_import_registry(name))?
                {
                    return Ok(cached);
                }
                if let Some(cached) = self.lookup_sys_modules(name)? {
                    return Ok(cached);
                }
                let active_parent = validate_import_cache_entry(
                    parent,
                    self.lookup_internal_import_registry(parent),
                )?
                .or(self.lookup_sys_modules(parent)?)
                .unwrap_or(loaded_parent);
                match self.get_attr(&active_parent, "__path__") {
                    Ok(p) => (Some(active_parent), p),
                    Err(err) if err.class_name_is("AttributeError") => {
                        if crate::builtin_modules::is_builtin_module(name) {
                            (Some(active_parent), Value::none())
                        } else {
                            return Err(PyError::import_error(
                                "ModuleNotFoundError",
                                format!("No module named '{name}'; '{parent}' is not a package"),
                                Some(name.to_string()),
                            ));
                        }
                    }
                    Err(err) => return Err(err),
                }
            } else {
                (None, Value::none())
            };

            for finder in meta_path_finders {
                let find_spec_fn = match self.get_attr(&finder, "find_spec") {
                    Ok(f) => f,
                    Err(e) if e.class_name_is("AttributeError") => continue,
                    Err(e) => return Err(e),
                };
                let spec = self.call_function_expanded(
                    find_spec_fn,
                    &[
                        ExpandedCallArg {
                            name: None,
                            value: Value::string(name),
                        },
                        ExpandedCallArg {
                            name: None,
                            value: meta_parent_path.clone(),
                        },
                        ExpandedCallArg {
                            name: None,
                            value: Value::none(),
                        },
                    ],
                )?;
                if !spec.is_none() {
                    return self.load_module_from_spec(name, spec, parent_name, meta_parent_mod);
                }
            }
        }

        // 2. Built-in modules — declared in
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
                if self.cold.runtime_config.verbose > 0 {
                    eprintln!("import '{name}' # builtin");
                }
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
                // Parent-package identity fix-up: if a built-in module binds a
                // submodule-shaped attr that is also a registered built-in
                // module, ensure the attr points at the cached `sys.modules`
                // entry so `pkg.submod is sys.modules["pkg.submod"]` matches
                // CPython.
                if let ValueKind::PyModule(m) = val.kind() {
                    let submodule_attrs: Vec<(String, Value)> = {
                        let snapshot = m.borrow().attrs_snapshot();
                        snapshot
                            .into_iter()
                            .filter(|(attr_name, attr_val)| {
                                // Only consider attrs that are themselves
                                // PyModules — primitive constants stay as-is.
                                if !matches!(attr_val.kind(), ValueKind::PyModule(_)) {
                                    return false;
                                }
                                // And only if there's a registered built-in
                                // by the dotted name — otherwise leave it.
                                let dotted = format!("{name}.{attr_name}");
                                crate::builtin_modules::is_builtin_module(&dotted)
                            })
                            .collect()
                    };
                    for (attr_name, attr_val) in submodule_attrs {
                        let dotted = format!("{name}.{attr_name}");
                        // Recursive load goes through the cache, so the
                        // first such call (whether triggered here or by an
                        // explicit `import os.path`) wins and subsequent
                        // accesses share its identity.
                        let cached_submodule = self.load_module(&dotted)?;
                        if !values_are_identical(&attr_val, &cached_submodule) {
                            m.borrow_mut().insert_attr(attr_name, cached_submodule);
                        }
                    }
                }
                // Dotted built-in module parent fix-up: importing a submodule
                // directly (e.g. `importlib.import_module("urllib.parse")` or
                // `import importlib.util as u`) ensures its built-in parent
                // package is also loaded in `sys.modules` and has this
                // submodule bound on it, matching CPython 3.12.
                if let Some((parent, leaf)) = name.rsplit_once('.')
                    && crate::builtin_modules::is_builtin_module(parent)
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

        // 3. Path-based lookup (`sys.path_hooks`, `sys.path_importer_cache`,
        // filesystem packages `<pkg>/__init__.py`, modules `<mod>.py`, and
        // PEP 420 implicit namespace packages).
        // Dotted names load parent packages first and search each child inside
        // its parent package's `__path__` (never falling back to `sys.path`).
        let (parent_module_value, search_entries) = if let Some(parent) = parent_name {
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
            let Some(entries) = self.extract_package_search_entries(&active_parent)? else {
                return Err(PyError::import_error(
                    "ModuleNotFoundError",
                    format!("No module named '{name}'; '{parent}' is not a package"),
                    Some(name.to_string()),
                ));
            };
            (Some(active_parent), entries)
        } else {
            (None, self.user_module_search_entries()?)
        };

        let mut filesystem_target: Option<FilesystemModuleTarget> = None;
        let mut namespace_dirs: Vec<String> = Vec::new();

        for entry_val in &search_entries {
            let Some(entry_str) = self.coerce_search_path_entry(entry_val)? else {
                continue;
            };
            match self.lookup_path_hook_finder(&entry_str)? {
                PathEntryFinderLookup::CachedNone => continue,
                PathEntryFinderLookup::Finder(finder) => {
                    let find_spec_fn = match self.get_attr(&finder, "find_spec") {
                        Ok(f) => f,
                        Err(e) if e.class_name_is("AttributeError") => continue,
                        Err(e) => return Err(e),
                    };
                    let spec = self.call_function_expanded(
                        find_spec_fn,
                        &[
                            ExpandedCallArg {
                                name: None,
                                value: Value::string(name),
                            },
                            ExpandedCallArg {
                                name: None,
                                value: Value::none(),
                            },
                        ],
                    )?;
                    if !spec.is_none() {
                        let loader = self
                            .get_attr(&spec, "loader")
                            .unwrap_or_else(|_| Value::none());
                        if !loader.is_none() {
                            return self.load_module_from_spec(
                                name,
                                spec,
                                parent_name,
                                parent_module_value,
                            );
                        }
                        if let Ok(smsl) = self.get_attr(&spec, "submodule_search_locations")
                            && !smsl.is_none()
                            && let Ok(locs) = self.collect_iterable(&smsl)
                        {
                            for loc in locs {
                                if let Some(loc_str) = self.coerce_search_path_entry(&loc)? {
                                    namespace_dirs.push(loc_str);
                                }
                            }
                        }
                    }
                }
                PathEntryFinderLookup::DefaultFilesystem => {
                    let dir = PathBuf::from(&entry_str);
                    if let Some(target) = find_filesystem_module_in_dir(&dir, leaf_name) {
                        filesystem_target = Some(target);
                        break;
                    }
                    let ns_dir = dir.join(leaf_name);
                    if ns_dir.is_dir() {
                        let abs_ns = std::path::absolute(&ns_dir)
                            .unwrap_or(ns_dir)
                            .to_string_lossy()
                            .into_owned();
                        namespace_dirs.push(abs_ns);
                    }
                }
            }
        }

        if let Some(target) = filesystem_target {
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
            let (program, linenos) =
                self.parse_source_to_stmts_with_warnings(&src, &module_filename)?;
            // Subinterpreter shares the same module_cache so results are visible to parent
            let sub_cold = Box::new(InterpreterColdState {
                script_dir: self.cold.script_dir.clone(),
                script_filename: Some(std::sync::Arc::from(module_filename.as_str())),
                // Imported modules run in the top-level script's process
                // context, so a later `import sys` must see the original
                // command-line arguments rather than an empty vector.
                script_argv: self.cold.script_argv.clone(),
                module_cache: Rc::clone(&self.cold.module_cache),
                bootstrap_module_registry: self.cold.bootstrap_module_registry.clone(),
                runtime_config: Rc::clone(&self.cold.runtime_config),
                // One `Rc` shares every lazily-initialized optional feature
                // state (gc/atexit/warnings/sys/...) with the parent (#3296).
                ..InterpreterColdState::new_shared(
                    std::sync::Arc::clone(&self.cold.gil_state),
                    false,
                    Rc::clone(&self.cold.feature_states),
                )
            });
            let mut sub = Interpreter::new_worker(
                Environment::new(None),
                get_int_max_str_digits(self),
                self.recursion_limit,
                sub_cold,
            );
            sub.vm_hooks_flags = self.vm_hooks_flags;
            if self.cold.runtime_config.verbose > 0 {
                eprintln!("import '{name}' # from '{module_filename}'");
            }
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

        if !namespace_dirs.is_empty() {
            return self.create_namespace_package(
                name,
                namespace_dirs,
                parent_name,
                parent_module_value,
            );
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
        if let Some(source) = match module_name {
            "timeit" => Some("import sys, timeit\nsys.exit(timeit.main())\n"),
            "dis" => Some("import dis\ndis.main()\n"),
            "site" => Some("import site\nsite._script()\n"),
            "pydoc" => Some("import pydoc\npydoc.cli()\n"),
            "json.tool" => Some("import json.tool\njson.tool.main()\n"),
            "venv" | "venv.__main__" => Some(
                "import sys, venv\nrc = 1\ntry:\n    venv.main()\n    rc = 0\nexcept Exception as e:\n    print('Error: %s' % e, file=sys.stderr)\nsys.exit(rc)\n",
            ),
            "ensurepip" | "ensurepip.__main__" => {
                Some("import sys, ensurepip\nsys.exit(ensurepip._main())\n")
            }
            "ensurepip._uninstall" => {
                Some("import sys, ensurepip._uninstall\nsys.exit(ensurepip._uninstall._main())\n")
            }
            _ => None,
        } {
            return Ok(MainModuleTarget::Builtin {
                virtual_path: format!("<frozen {module_name}>"),
                source,
            });
        }
        if crate::builtin_modules::is_builtin_module(module_name) {
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
                Some(main_target) if main_target.package_dir.is_none() => {
                    Ok(MainModuleTarget::File {
                        file_path: main_target.file_path,
                        package_name: pkg_name.to_string(),
                    })
                }
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
                Ok(MainModuleTarget::File {
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
                Ok(MainModuleTarget::File {
                    file_path: target.file_path,
                    package_name: String::new(),
                })
            }
        }
    }
}
