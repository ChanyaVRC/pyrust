impl Interpreter {
    /// Probe and fill the unified `LoadGlobal` resolution cache around the
    /// namespace-owned slow path.
    pub(super) fn load_global_cached(
        &mut self,
        code: &crate::bytecode::FnCode,
        name_index: u16,
    ) -> Result<Value> {
        let slot = name_index as usize;
        let (
            current_namespace_id,
            current_environment_version,
            current_structure_version,
            _current_mirror_epoch,
            _cache_disabled,
        ) = {
            let env = self.env.borrow();
            let snapshot = env.namespace_cache_snapshot();
            // Once globals() has exposed the backing dict, arbitrary alias methods
            // (e.g. update/clear) can mutate it without an interpreter callback.
            // Keep those uncommon namespaces uncached instead of installing a
            // process-wide dict mutation watcher on every normal program.
            if !snapshot.4 {
                let cached = code.global_cache.borrow().get(slot).and_then(|entry| {
                    entry.lookup_with_fastlocal(
                        snapshot.0,
                        snapshot.1,
                        snapshot.2,
                        snapshot.3,
                        |mirror_epoch, local_index, register| {
                            env.namespace_fastlocal_cached_value(
                                mirror_epoch,
                                local_index,
                                register,
                            )
                        },
                    )
                });
                if let Some(value) = cached {
                    return Ok(value);
                }
            }
            snapshot
        };

        let name = code.names.get(slot).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: name index {name_index} out of range"
            ))
        })?;
        let namespace = self.prepare_global_namespace(name);
        let resolution = self.resolve_global_uncached(
            name,
            comp_read_is_free(code, name),
            current_environment_version,
            &namespace,
        )?;
        let cache_interest_mask = code.global_cache_interest_masks[slot];

        match resolution.cache {
            GlobalResolutionCache::None => {}
            GlobalResolutionCache::Environment(version)
                if version != u64::MAX && namespace.cacheable() =>
            {
                self.env
                    .borrow()
                    .register_namespace_value_cache_interest(cache_interest_mask);
                code.global_cache.borrow_mut()[slot] =
                    GlobalCacheEntry::environment(current_namespace_id, version, &resolution.value);
            }
            GlobalResolutionCache::ScriptFastLocal {
                environment_version,
                mirror_epoch,
                local_index,
                register,
            } if namespace.cacheable() => {
                self.env
                    .borrow()
                    .register_namespace_value_cache_interest(cache_interest_mask);
                code.global_cache.borrow_mut()[slot] = GlobalCacheEntry::script_fastlocal(
                    current_namespace_id,
                    environment_version,
                    mirror_epoch,
                    local_index,
                    register,
                    &resolution.value,
                );
            }
            GlobalResolutionCache::Builtin(provider_state)
                if current_structure_version != u64::MAX && namespace.cacheable() =>
            {
                self.env
                    .borrow()
                    .register_namespace_fallback_cache_interest(cache_interest_mask);
                code.global_cache.borrow_mut()[slot] = GlobalCacheEntry::builtin_module(
                    current_namespace_id,
                    current_structure_version,
                    provider_state,
                    &resolution.value,
                );
            }
            GlobalResolutionCache::Environment(_)
            | GlobalResolutionCache::ScriptFastLocal { .. }
            | GlobalResolutionCache::Builtin(_) => {}
        }
        Ok(resolution.value)
    }

    /// Resolve one compiler-known class-body name without deoptimizing the
    /// ordinary class path: a class whose namespace has not escaped performs
    /// only the encoded register read. Once exposed, the dict is authoritative;
    /// a missing key follows LOAD_NAME's module/builtins fallback.
    pub(super) fn load_class_name(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &RegSlice,
        local: crate::bytecode::Reg,
        name_index: u16,
    ) -> Result<Value> {
        if let Some(namespace) = active_live_class_namespace(self) {
            let name = code.names.get(name_index as usize).ok_or_else(|| {
                PyError::Runtime(format!(
                    "bytecode error: name index {name_index} out of range"
                ))
            })?;
            if let Some(value) = namespace
                .dict_with(|dict| dict.get(&StrKey(name)).cloned())
                .flatten()
            {
                return Ok(value);
            }
            return self.load_global_cached(code, name_index);
        }

        if local == crate::bytecode::NO_CLASS_LOCAL {
            return self.load_global_cached(code, name_index);
        }

        let value = regs.get(local as usize).ok_or_else(|| {
            PyError::Runtime(format!(
                "bytecode error: class local register {local} out of range"
            ))
        })?;
        if value.is_unset() {
            self.load_global_cached(code, name_index)
        } else {
            Ok(value.clone())
        }
    }

    /// Class-body `LOAD_CLASSDEREF` (issue #452): consult the live class
    /// namespace dict first when `locals()`/`f_locals` has materialized, then
    /// read `cells[cell_idx]`. An empty cell raises free-variable `NameError`
    /// (never falling back to module globals).
    pub(super) fn load_class_deref(
        &mut self,
        code: &crate::bytecode::FnCode,
        cells: CellSlice,
        cell_idx: u16,
    ) -> Result<Value> {
        let meta = code.cell_slots[cell_idx as usize];
        if let Some(namespace) = active_live_class_namespace(self)
            && let Some(value) = namespace
                .dict_with(|dict| dict.get(&StrKey(meta.name.as_str())).cloned())
                .flatten()
        {
            return Ok(value);
        }
        cells.as_slice()[cell_idx as usize]
            .get()
            .ok_or_else(|| unbound_binding_error(meta.name.as_str(), true))
    }
}
