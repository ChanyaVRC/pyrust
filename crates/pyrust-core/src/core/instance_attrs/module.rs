/// Live namespace linkage used only by source-backed filesystem modules.
///
/// `globals` is the exact dictionary also retained by the root environment and
/// returned by functions' `__globals__`. The environment link is deliberately
/// weak: circular imports may place module objects in each other's namespaces,
/// so a strong link here would add an uncollectable
/// `PyModule -> Environment -> PyModule` ownership cycle. A captured function
/// keeps its own environment alive; a module containing only data remains
/// usable from the strongly-owned globals dictionary after the loader
/// interpreter and environment have gone away. As with an ordinary exposed
/// `globals()` dictionary, storing a function back into its own globals can
/// still form a Python-level dict/function/environment cycle; collecting that
/// general object-graph cycle is outside this module state and is not made
/// worse by a second strong module-to-environment edge.
#[derive(Debug, Clone)]
pub struct FilesystemModuleNamespace {
    environment: Weak<RefCell<Environment>>,
    globals: Value,
}

impl FilesystemModuleNamespace {
    fn new(environment: &EnvRef, globals: Value) -> Self {
        debug_assert!(globals.is_dict());
        Self {
            environment: Rc::downgrade(environment),
            globals,
        }
    }

    #[inline]
    pub fn environment(&self) -> Option<EnvRef> {
        self.environment.upgrade()
    }

    #[inline]
    pub fn globals(&self) -> Value {
        self.globals.clone()
    }
}

#[derive(Debug)]
pub struct PyModule {
    pub name: String,
    pub attrs: HashMap<String, Value>,
    mutation_state: ModuleMutationState,
    /// Boxed so the built-in-module common case pays only one nullable pointer
    /// and continues to use its existing direct `attrs` HashMap fast path.
    filesystem_namespace: Option<Box<FilesystemModuleNamespace>>,
}

impl PyModule {
    pub fn new(name: String, attrs: HashMap<String, Value>) -> Self {
        Self {
            name,
            attrs,
            mutation_state: ModuleMutationState::fresh(),
            filesystem_namespace: None,
        }
    }

    /// Attach this source-backed module to the environment executing its body.
    ///
    /// The root's pre-existing globals `Value` becomes the module's sole live
    /// Python dictionary. No attribute snapshot is harvested after execution.
    pub fn attach_filesystem_namespace(&mut self, environment: &EnvRef) {
        let globals = environment.borrow().namespace_globals();
        environment
            .borrow()
            .configure_filesystem_module_namespace(self.mutation_state());
        self.attrs.clear();
        self.filesystem_namespace = Some(Box::new(FilesystemModuleNamespace::new(
            environment,
            globals,
        )));
    }

    #[inline]
    pub fn filesystem_namespace(&self) -> Option<FilesystemModuleNamespace> {
        self.filesystem_namespace.as_deref().cloned()
    }

    /// Read one attribute from the module's authoritative namespace.
    #[inline]
    pub fn get_attr_value(&self, name: &str) -> Option<Value> {
        if let Some(namespace) = self.filesystem_namespace.as_deref() {
            return namespace
                .globals
                .dict_with(|dict| dict.get(&crate::object_model::StrKey(name)).cloned())
                .flatten();
        }
        self.attrs.get(name).cloned()
    }

    /// Snapshot string-keyed attributes for generic reflection paths.
    ///
    /// Filesystem module dictionaries may contain non-string keys through a
    /// direct `__dict__` alias; those are visible in the dictionary itself but
    /// are not Python attributes and are intentionally omitted here.
    pub fn attrs_snapshot(&self) -> HashMap<String, Value> {
        if let Some(namespace) = self.filesystem_namespace.as_deref() {
            return namespace
                .globals
                .dict_with(|dict| {
                    dict.iter()
                        .filter_map(|(key, value)| {
                            let PyKey::Str(key) = key else {
                                return None;
                            };
                            key.as_str().map(|name| (name.to_string(), value.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        self.attrs.clone()
    }

    /// Return the stable provider token used by module-backed namespace caches.
    #[inline]
    pub fn mutation_state(&self) -> ModuleMutationState {
        self.mutation_state.clone()
    }

    /// Insert or replace one module attribute and invalidate namespace caches.
    #[inline]
    pub fn insert_attr(&mut self, name: String, value: Value) -> Option<Value> {
        if let Some(namespace) = self.filesystem_namespace.as_deref() {
            let previous = namespace
                .globals
                .dict_insert(crate::object_model::PyKey::str_from(&name), value.clone())
                .expect("filesystem module globals must remain a dict");
            if let Some(environment) = namespace.environment() {
                let mut environment = environment.borrow_mut();
                environment.record_namespace_env_binding(&name);
                environment.values.insert(&name, value.clone());
                environment.synchronize_namespace_fastlocal_binding(&name, &value, None);
                environment.bump_namespace_structure_version();
            }
            self.mutation_state.bump();
            return previous;
        }
        let previous = self.attrs.insert(name, value);
        self.mutation_state.bump();
        previous
    }

    /// Remove one module attribute and invalidate namespace caches when found.
    #[inline]
    pub fn remove_attr(&mut self, name: &str) -> Option<Value> {
        if let Some(namespace) = self.filesystem_namespace.as_deref() {
            let removed = namespace
                .globals
                .dict_shift_remove(&crate::object_model::PyKey::str_from(name))
                .expect("filesystem module globals must remain a dict");
            if removed.is_some() {
                if let Some(environment) = namespace.environment() {
                    let mut environment = environment.borrow_mut();
                    environment.values.remove(name);
                    environment.synchronize_namespace_fastlocal_binding(
                        name,
                        &Value::unset(),
                        None,
                    );
                    environment.bump_namespace_structure_version();
                }
                self.mutation_state.bump();
            }
            return removed;
        }
        let removed = self.attrs.remove(name);
        if removed.is_some() {
            self.mutation_state.bump();
        }
        removed
    }

    /// Replace the complete module namespace while preserving its storage kind.
    #[inline]
    pub fn replace_attrs(&mut self, attrs: HashMap<String, Value>) {
        if let Some(namespace) = self.filesystem_namespace.as_deref() {
            namespace
                .globals
                .dict_clear()
                .expect("filesystem module globals must remain a dict");
            if let Some(environment) = namespace.environment() {
                environment.borrow_mut().values.clear();
            }
            for (name, value) in attrs {
                let _ = namespace
                    .globals
                    .dict_insert(crate::object_model::PyKey::str_from(&name), value.clone());
                if let Some(environment) = namespace.environment() {
                    let mut environment = environment.borrow_mut();
                    environment.record_namespace_env_binding(&name);
                    environment.values.insert(&name, value);
                }
            }
            if let Some(environment) = namespace.environment() {
                environment
                    .borrow()
                    .synchronize_namespace_fastlocals_from_mapping(&namespace.globals);
            }
            self.mutation_state.bump();
            return;
        }
        self.attrs = attrs;
        self.mutation_state.bump();
    }
}

impl Clone for PyModule {
    fn clone(&self) -> Self {
        // Cloning the struct creates a new module object, not a Python alias
        // (aliases clone the surrounding `Value`/`Rc`).  Give that object a
        // distinct provider identity rather than sharing cache generations.
        Self::new(self.name.clone(), self.attrs_snapshot())
    }
}
