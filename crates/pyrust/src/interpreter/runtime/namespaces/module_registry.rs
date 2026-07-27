// Import-registry ownership and lifecycle.
//
// Before `sys` exists, the Interpreter-owned dict is the bootstrap registry.
// Once the canonical `sys` module is loaded, its current `modules` attribute is
// authoritative. Resolving the attribute here (on the cold import/cache-miss
// path) makes `sys.modules = replacement` and `del sys.modules` visible across
// the short-lived child Interpreter used to execute an imported module, because
// parent and child already share the canonical module object through
// `module_cache`.
impl Interpreter {
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
    ) -> Result<(Value, Option<ModuleMutationState>)> {
        let system_module = self.module_cache.borrow().get("sys").cloned();
        let Some(system_module) = system_module else {
            return Ok((self.bootstrap_module_registry.clone(), None));
        };
        let ValueKind::PyModule(system_module) = system_module.kind() else {
            debug_assert!(false, "the internal sys cache entry must be a module");
            return Ok((self.bootstrap_module_registry.clone(), None));
        };

        let (registry, owner_state) = {
            let system_module = system_module.borrow();
            let registry = system_module.get_attr_value("modules").ok_or_else(|| {
                PyError::named(
                    "AttributeError",
                    "module 'sys' has no attribute 'modules'".to_string(),
                )
            })?;
            (registry, system_module.mutation_state())
        };
        if !registry.is_dict() {
            return Err(PyError::named(
                "AttributeError",
                format!(
                    "'{}' object has no attribute 'get'",
                    pyrust_core::builtin_type_name(&registry)
                ),
            ));
        }
        Ok((registry, Some(owner_state)))
    }
}
