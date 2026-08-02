// Import-registry ownership and lifecycle.
//
// Before `sys` exists, the Interpreter-owned dict is both the bootstrap
// registry and the import system's internal cache. Once `sys` is loaded, its
// initial `modules` attribute aliases that dict. Python may later rebind the
// attribute, but CPython's internal cache keeps the original dict: an entry
// already present there wins before importlib consults the replacement mapping,
// while names absent from it are resolved through the current visible mapping.
// The generic visible-registry operations live here; `modules.rs` owns that
// per-name priority decision.

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportRegistryKind {
    ExactDict,
    Protocol,
}

/// Select the allocation-free exact-dict path or the observable mapping
/// protocol path.
///
/// CPython does not require `sys.modules` to inherit `dict`: lookup calls
/// `get(name, sentinel)`, while publication and failed-import cleanup use the
/// mapping assignment/deletion slots. Validation therefore belongs to the
/// operation being attempted, not to a nominal registry class check.
fn classify_import_registry(registry: &Value) -> ImportRegistryKind {
    if registry.is_dict() {
        ImportRegistryKind::ExactDict
    } else {
        ImportRegistryKind::Protocol
    }
}

impl Interpreter {
    /// Read the import system's original exact-dict cache.
    ///
    /// This is deliberately distinct from [`Self::lookup_import_registry`].
    /// Rebinding `sys.modules` changes the mapping importlib sees, but not the
    /// interpreter-level cache already populated through the original dict.
    /// Direct mutation of that original dict remains immediately observable
    /// because this reads its shared backing on every probe.
    #[inline]
    pub(super) fn lookup_internal_import_registry(&self, name: &str) -> Option<Value> {
        self.bootstrap_module_registry
            .dict_with(|registry| registry.get(&StrKey(name)).cloned())
            .flatten()
    }

    /// Allocation-free exact-dict cache probe used by every import lookup.
    ///
    /// The general resolver returns a `Value`, which would need a temporary
    /// `OpaqueSlot` when upgrading a weak dict. Reads can borrow the shared
    /// backing directly while retaining the same sys identity/generation
    /// guards.
    #[inline]
    fn cached_import_registry_dict(&self) -> Option<Rc<RefCell<PyDict>>> {
        let module_cache = self.module_cache.borrow();
        let system_module = module_cache.get("sys")?;
        let ValueKind::PyModule(system_module) = system_module.kind() else {
            return None;
        };
        let cache = self.import_module_registry_cache.borrow();
        let cached = cache.as_ref()?;
        if cached.system_module.as_ptr() != Rc::as_ptr(system_module)
            || !cached
                .registry_owner_state
                .matches_cache_version(cached.registry_owner_version)
        {
            return None;
        }
        cached.registry.upgrade_dict()
    }

    #[inline]
    pub(crate) fn import_module_registry(&self) -> Result<Value> {
        self.import_module_registry_with_owner_state()
            .map(|(registry, _)| registry)
    }

    /// Resolve the active registry together with the generation of the
    /// canonical `sys` module that owns its binding.
    ///
    /// `None` means `sys` has not been imported yet. Callers may still use the
    /// bootstrap registry, but a long-lived equality cache must not be installed
    /// until there is an owner generation capable of observing a later
    /// `sys.modules` replacement.
    pub(super) fn import_module_registry_with_owner_state(
        &self,
    ) -> Result<(Value, Option<CollectionMutationState>)> {
        let system_module = {
            let module_cache = self.module_cache.borrow();
            let Some(system_module) = module_cache.get("sys") else {
                return Ok((self.bootstrap_module_registry.clone(), None));
            };
            let ValueKind::PyModule(system_module) = system_module.kind() else {
                debug_assert!(false, "the internal sys cache entry must be a module");
                return Ok((self.bootstrap_module_registry.clone(), None));
            };
            if let Some(cached) = self.import_module_registry_cache.borrow().as_ref()
                && cached.system_module.as_ptr() == Rc::as_ptr(system_module)
                && cached
                    .registry_owner_state
                    .matches_cache_version(cached.registry_owner_version)
                && let Some(registry) = cached.registry.upgrade()
            {
                return Ok((registry, Some(cached.registry_owner_state.clone())));
            }
            Rc::clone(system_module)
        };

        let (visible_registry, owner_state) = {
            let system_module = system_module.borrow();
            let registry = system_module.get_attr_value("modules").ok_or_else(|| {
                PyError::named(
                    "AttributeError",
                    "module 'sys' has no attribute 'modules'".to_string(),
                )
            })?;
            let owner_state = system_module
                .live_namespace()
                .and_then(|namespace| namespace.dict_iteration_mutation_state());
            (registry, owner_state)
        };
        // Protocol registries may implement lookup through arbitrary Python
        // code and have no stable mutation generation. Exact dicts alone may
        // seed the weak steady-state cache.
        let owner_state = (classify_import_registry(&visible_registry)
            == ImportRegistryKind::ExactDict)
            .then_some(owner_state)
            .flatten();
        if let (Some(owner_state), Some(registry_cache)) = (
            owner_state.as_ref(),
            pyrust_core::WeakValueCache::new(&visible_registry),
        ) && let Some(registry_owner_version) = owner_state.cache_version()
        {
            *self.import_module_registry_cache.borrow_mut() = Some(CachedImportModuleRegistry {
                system_module: Rc::downgrade(&system_module),
                registry_owner_state: owner_state.clone(),
                registry_owner_version,
                registry: registry_cache,
            });
            return Ok((visible_registry, Some(owner_state.clone())));
        }
        *self.import_module_registry_cache.borrow_mut() = None;
        Ok((visible_registry, owner_state))
    }

    /// Read one import-cache entry.
    ///
    /// Exact dicts stay on the raw storage path. Any non-exact provider,
    /// including a dict subclass, is cold and deliberately calls its observable
    /// `get(name, default)` method rather than bypassing the mapping protocol.
    pub(super) fn lookup_import_registry(&mut self, name: &str) -> Result<Option<Value>> {
        if let Some(registry) = self.cached_import_registry_dict() {
            return Ok(registry.borrow().get(&StrKey(name)).cloned());
        }
        let registry = self.import_module_registry()?;
        match classify_import_registry(&registry) {
            ImportRegistryKind::ExactDict => {
                return Ok(registry
                    .dict_with(|dict| dict.get(&StrKey(name)).cloned())
                    .flatten());
            }
            ImportRegistryKind::Protocol => {}
        }

        let missing = Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: object_class_singleton(),
            attrs: InstanceAttrs::new(),
        })));
        let getter = self.get_attr(&registry, "get")?;
        let result = self.call_function_expanded(
            getter,
            &[
                ExpandedCallArg {
                    name: None,
                    value: Value::string(name),
                },
                ExpandedCallArg {
                    name: None,
                    value: missing.clone(),
                },
            ],
        )?;
        Ok((!values_are_identical(&result, &missing)).then_some(result))
    }

    /// Publish one entry through the active registry's assignment protocol.
    pub(super) fn insert_import_registry(&mut self, name: &str, module: Value) -> Result<()> {
        let registry = self.import_module_registry()?;
        match classify_import_registry(&registry) {
            ImportRegistryKind::ExactDict => {
                registry.dict_insert(PyKey::str_from(name), module)?;
                Ok(())
            }
            ImportRegistryKind::Protocol => {
                let method = lookup_value_special_method(&registry, "__setitem__")
                    .transpose()?
                    .ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!(
                                "'{}' object does not support item assignment",
                                pyrust_core::builtin_type_name(&registry)
                            ),
                        )
                    })?;
                invoke_class_method(
                    self,
                    method,
                    registry,
                    &[
                        ExpandedCallArg {
                            name: None,
                            value: Value::string(name),
                        },
                        ExpandedCallArg {
                            name: None,
                            value: module,
                        },
                    ],
                )?;
                Ok(())
            }
        }
    }

    /// Remove one entry through the active registry's deletion protocol.
    pub(super) fn remove_import_registry(&mut self, name: &str) -> Result<()> {
        let registry = self.import_module_registry()?;
        match classify_import_registry(&registry) {
            ImportRegistryKind::ExactDict => {
                registry.dict_shift_remove(&PyKey::str_from(name))?;
                Ok(())
            }
            ImportRegistryKind::Protocol => {
                let method = lookup_value_special_method(&registry, "__delitem__")
                    .transpose()?
                    .ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            format!(
                                "'{}' object does not support item deletion",
                                pyrust_core::builtin_type_name(&registry)
                            ),
                        )
                    })?;
                invoke_class_method(
                    self,
                    method,
                    registry,
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string(name),
                    }],
                )?;
                Ok(())
            }
        }
    }
}
