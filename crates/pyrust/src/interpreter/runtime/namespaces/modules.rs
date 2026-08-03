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
            return Err(PyError::Runtime(format!(
                "{module_name}: active module is not a module"
            )));
        };
        let (module_state, class) = {
            let module = module.borrow();
            let class = module.get_attr_value(attribute_name).ok_or_else(|| {
                PyError::Runtime(format!("{module_name}: missing {attribute_name}"))
            })?;
            let ValueKind::PyClass(class) = class.kind() else {
                return Err(PyError::Runtime(format!(
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
                if !values_are_identical(&registry, &self.bootstrap_module_registry) {
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
            self.module_class_cache
                .get_or_insert_with(|| Box::new(ModuleClassCache::default()))
                .entries[slot.0] = Some(entry);
        } else if let Some(cache) = self.module_class_cache.as_mut() {
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
        self.module_cache.borrow_mut().remove(name);
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
            self.bootstrap_module_registry.clone(),
        );
    }

    /// Snapshot the import roots currently exposed through `sys.path`.
    /// Before `sys` is imported, preserve the legacy script-directory lookup
    /// so a script can still import a sibling module as its very first import.
    fn user_module_search_dirs(&self) -> Vec<PathBuf> {
        let sys_path = self.module_cache.borrow().get("sys").and_then(|module| {
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

        self.script_dir.iter().cloned().collect()
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
        let present_internally = self.module_cache.borrow().contains_key(name);
        if present_internally {
            self.module_cache.borrow_mut().remove(name);
        }
        // Built-in modules — declared in
        // `crates/pyrust/src/builtin_modules/mod.rs::pyrust_builtin_modules!`.
        // Adding a new module is a single-line edit there; this file
        // never has to change.
        let builtin = crate::builtin_modules::load_builtin_module(name);
        if let Some(val) = builtin {
            crate::builtin_modules::prepare_builtin_module(name, self, &val);
            self.module_cache
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
                        let borrowed = m.borrow();
                        borrowed
                            .attrs
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
                Ok(())
            })();
            if let Err(error) = initialization {
                self.unregister_failed_module(name);
                return Err(error);
            }
            return Ok(val);
        }
        // User .py file: look for <name>.py in the active sys.path roots.
        // Convert dotted name to path: "foo.bar" -> "foo/bar.py".
        let rel_path = name.replace('.', "/") + ".py";
        for dir in self.user_module_search_dirs() {
            let full_path = dir.join(&rel_path);
            if full_path.exists() {
                let src = std::fs::read_to_string(&full_path).map_err(|e| {
                    PyError::Runtime(format!("failed to read '{}': {e}", full_path.display()))
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
                    script_dir: self.script_dir.clone(),
                    script_filename: Some(std::sync::Arc::from(module_filename.as_str())),
                    // Imported modules run in the top-level script's process
                    // context, so a later `import sys` must see the original
                    // command-line arguments rather than an empty vector.
                    script_argv: self.script_argv.clone(),
                    module_cache: Rc::clone(&self.module_cache),
                    bootstrap_module_registry: self.bootstrap_module_registry.clone(),
                    // Filesystem imports execute through a child object but
                    // remain inside the parent's Python interpreter. Sharing
                    // warnings state mirrors the singleton `warnings` module
                    // already shared through `module_cache`/`sys.modules`.
                    warnings_state: Rc::clone(&self.warnings_state),
                    // This child is an implementation detail of importing a
                    // module into the same Python interpreter. Start with the
                    // parent's integer conversion policy; changes are copied
                    // back below even if module execution raises.
                    int_max_str_digits: get_int_max_str_digits(self),
                    // Imported Python modules execute in a child Interpreter
                    // but share the parent's recursion policy.
                    recursion_limit: self.recursion_limit,
                    ..Default::default()
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
                    // Seed the import identity before execution. The script
                    // dunder initializer uses get-or-insert and therefore
                    // preserves these filesystem-module values.
                    //
                    // `__doc__` is seeded between them purely for position:
                    // these keys are inserted before the script initializer
                    // runs, so they fix the head of `list(vars(module))`, and
                    // CPython's module dict starts `__name__`, `__doc__`,
                    // `__package__` (issue #2918 — the same head the built-in
                    // module paths now produce). A module docstring overwrites
                    // the `None` in place when the body executes.
                    module.insert_attr("__name__".to_string(), Value::string(name));
                    module.insert_attr("__doc__".to_string(), Value::none());
                    module.insert_attr("__package__".to_string(), Value::string(""));
                }
                let module = Value::py_module(Rc::clone(&module_rc));
                // Register before executing the body so a circular import sees
                // this partial object. Script writes are mirrored into its live
                // backing immediately, making already-executed bindings visible.
                self.module_cache
                    .borrow_mut()
                    .insert(name.to_string(), module.clone());
                if let Err(error) = self.register_in_sys_modules(name, &module) {
                    self.unregister_failed_module(name);
                    return Err(error);
                }
                // call_depth is thread_local — the child automatically shares
                // the active host-stack counter.
                let execution_result =
                    sub.exec_import_program_with_linenos(&program, &linenos, &src);
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
                return Ok(module);
            }
        }
        // Relative imports (module name starts with '.') cannot be resolved in
        // pyrust's package-less runtime.  CPython 3.12 raises ImportError (not
        // ModuleNotFoundError) with a specific message in this case.
        if name.starts_with('.') {
            return Err(PyError::import_error(
                "ImportError",
                "attempted relative import with no known parent package".to_string(),
                None,
            ));
        }
        Err(PyError::import_error(
            "ModuleNotFoundError",
            format!("No module named '{name}'"),
            Some(name.to_string()),
        ))
    }
}
