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
            current_mirror_epoch,
            cache_disabled,
        ) = self.env.borrow().namespace_cache_snapshot();

        // Once globals() has exposed the backing dict, arbitrary alias methods
        // (e.g. update/clear) can mutate it without an interpreter callback.
        // Keep those uncommon namespaces uncached instead of installing a
        // process-wide dict mutation watcher on every normal program.
        if !cache_disabled {
            let cached = code.global_cache.borrow().get(slot).and_then(|entry| {
                entry.lookup_with_fastlocal(
                    current_namespace_id,
                    current_environment_version,
                    current_structure_version,
                    current_mirror_epoch,
                    |mirror_epoch, local_index, register| {
                        self.env.borrow().namespace_fastlocal_cached_value(
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
}
