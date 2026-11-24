impl Environment {
    /// Bitflags returned by [`Self::prepare_namespace_module_write_flags`] so
    /// the hot `SyncModuleGlobal` inline path avoids constructing/dropping an
    /// `Option<Value>` or borrowing `globals_provider` in the common case.
    pub const MODULE_WRITE_SYNC_GLOBALS: u8 = 1 << 0;
    pub const MODULE_WRITE_SYNC_SIBLINGS: u8 = 1 << 1;
    pub const MODULE_WRITE_SYNC_ENV: u8 = 1 << 2;
    pub const MODULE_WRITE_TRACK_BINDING_ORDER: u8 = 1 << 3;

    /// Hot-path preparation for `SyncModuleGlobal`: advance interested cache
    /// generations and report which slow synchronization actions (if any) are
    /// needed via a compact bitmask.
    #[inline]
    pub fn prepare_namespace_module_write_flags(&self, name_mask: u64) -> u8 {
        let fallback_cache_interested =
            self.root_namespace.fallback_cache_interest.get() & name_mask == name_mask;
        let value_cache_interested =
            self.root_namespace.value_cache_interest.get() & name_mask == name_mask;
        if fallback_cache_interested {
            // A structure bump already advances the environment generation.
            // Keep the two invalidations in one operation when both cache
            // interest sets contain this name.
            self.root_namespace.bump_structure_version();
        } else if value_cache_interested {
            self.root_namespace.bump_environment_version();
        }
        let filesystem_module_backing = self.root_namespace.filesystem_module_backing.get();
        let cache_disabled = self.root_namespace.cache_disabled.get();
        let mut flags = 0u8;
        if filesystem_module_backing || cache_disabled {
            flags |= Self::MODULE_WRITE_SYNC_GLOBALS;
        }
        if !cache_disabled && self.root_namespace.active_mirror_count.get() > 1 {
            flags |= Self::MODULE_WRITE_SYNC_SIBLINGS;
        }
        if self.root_namespace.env_binding_interest.get() & name_mask == name_mask {
            flags |= Self::MODULE_WRITE_SYNC_ENV;
        }
        if self.root_namespace.has_binding_order.get() {
            flags |= Self::MODULE_WRITE_TRACK_BINDING_ORDER;
        }
        flags
    }

    /// Cold half of `SyncModuleGlobal` globals-provider mirroring: bump the
    /// attached filesystem module mutation token (if any) and return the
    /// Python-visible target mapping.
    pub fn namespace_module_write_target(&self) -> Option<Value> {
        let filesystem_module_backing = self.root_namespace.filesystem_module_backing.get();
        if filesystem_module_backing
            && let Some(module_mutation) = self
                .root_namespace
                .filesystem_module_mutation
                .borrow()
                .as_ref()
        {
            module_mutation.bump();
        }
        if !filesystem_module_backing && !self.root_namespace.cache_disabled.get() {
            return None;
        }

        let provider = self.root_namespace.globals_provider.borrow();
        match provider.as_ref() {
            Some(NamespaceGlobalsProvider::Normal(globals)) => Some(globals.clone()),
            Some(NamespaceGlobalsProvider::Explicit { globals, locals }) => {
                Some(locals.as_ref().unwrap_or(globals).clone())
            }
            None => {
                debug_assert!(
                    false,
                    "a mirrored namespace must initialize its globals provider before writes"
                );
                None
            }
        }
    }

    /// Prepare one module fast-local write and return the dictionary that must
    /// mirror it, if any.
    ///
    /// This is the hot `SyncModuleGlobal` operation: conditionally advancing
    /// cache generations and deciding whether a Python-visible globals/locals
    /// provider needs a write must share one root-state access. In the common
    /// ordinary script case the provider has never escaped, so return before
    /// borrowing or lazily allocating its dictionary.
    #[inline]
    pub fn prepare_namespace_module_write(&self, name_mask: u64) -> Option<Value> {
        let flags = self.prepare_namespace_module_write_flags(name_mask);
        if flags & Self::MODULE_WRITE_SYNC_GLOBALS == 0 {
            return None;
        }
        self.namespace_module_write_target()
    }

    #[inline]
    pub fn bump_namespace_structure_version(&self) {
        self.root_namespace.bump_structure_version();
    }

    /// Invalidate caches backed by the attached source module's attribute
    /// provider. Ordinary script roots have no token and make this a no-op.
    #[inline]
    pub fn bump_filesystem_module_mutation(&self) {
        if !self.root_namespace.filesystem_module_backing.get() {
            return;
        }
        if let Some(module_mutation) = self
            .root_namespace
            .filesystem_module_mutation
            .borrow()
            .as_ref()
        {
            module_mutation.bump();
        }
    }

    /// Return `(globals, optional separate locals, is_explicit)` for this
    /// namespace.  Normal namespace globals are allocated on first use.
    pub fn namespace_globals_provider(&self) -> (Value, Option<Value>, bool) {
        self.root_namespace.globals_provider_snapshot()
    }

    pub fn namespace_globals(&self) -> Value {
        self.root_namespace.globals_provider_snapshot().0
    }

    pub fn namespace_explicit_globals(&self) -> Option<Value> {
        let (globals, _, explicit) = self.root_namespace.globals_provider_snapshot();
        explicit.then_some(globals)
    }

    pub fn namespace_explicit_locals(&self) -> Option<Value> {
        let (globals, locals, explicit) = self.root_namespace.globals_provider_snapshot();
        explicit.then(|| locals.unwrap_or(globals))
    }

    pub fn namespace_is_explicit(&self) -> bool {
        self.root_namespace.globals_provider_snapshot().2
    }

    /// Attach caller-owned globals/locals to a fresh explicit exec/eval root.
    /// Explicit providers are permanently uncached because their dictionaries
    /// can be mutated through arbitrary aliases.
    pub fn configure_explicit_namespace(&self, globals: Value, locals: Option<Value>) {
        register_namespace_alias(&self.root_namespace, &globals);
        if let Some(locals) = &locals {
            register_namespace_alias(&self.root_namespace, locals);
        }
        *self.root_namespace.globals_provider.borrow_mut() =
            Some(NamespaceGlobalsProvider::Explicit { globals, locals });
        self.root_namespace.cache_disabled.set(true);
        self.root_namespace.recompute_sync_fast_check_mask();
    }

    /// Link a filesystem `PyModule` to this root namespace.
    ///
    /// The module keeps only a weak environment link, so circular imports do
    /// not add a strong `PyModule -> Environment -> PyModule` cycle. Functions
    /// retain their captured environment independently, while the module owns
    /// the shared globals dictionary for constant-only modules.
    pub fn configure_filesystem_module_namespace(&self, module_mutation: ModuleMutationState) {
        self.root_namespace.filesystem_module_backing.set(true);
        self.root_namespace.recompute_sync_fast_check_mask();
        *self.root_namespace.filesystem_module_mutation.borrow_mut() = Some(module_mutation);
    }

    /// Mark a normal module globals dict as Python-visible and return its stable
    /// backing object.
    ///
    /// The caller must finish the first fastlocal/env snapshot before calling
    /// [`Self::activate_namespace_globals_alias`]. Delaying registration keeps
    /// snapshot-building inserts from feeding an incomplete dict back into the
    /// live register mirror.
    pub fn expose_namespace_globals(&self) -> Value {
        let globals = self.root_namespace.globals_provider_snapshot().0;
        self.root_namespace.cache_disabled.set(true);
        self.root_namespace.recompute_sync_fast_check_mask();
        if let Some(module_mutation) = self
            .root_namespace
            .filesystem_module_mutation
            .borrow()
            .as_ref()
        {
            module_mutation.disable_cache();
        }
        globals
    }

    /// Activate storage-level mutation tracking after a normal globals dict's
    /// first exposure snapshot is complete.
    pub fn activate_namespace_globals_alias(&self, globals: &Value) {
        register_namespace_alias(&self.root_namespace, globals);
        // Once the live dict is authoritative it tracks insertion and deletion
        // order natively, so drop any pre-exposure binding-order journal.
        self.root_namespace.has_binding_order.set(false);
        self.root_namespace.recompute_sync_fast_check_mask();
        *self.root_namespace.binding_order.borrow_mut() = None;
    }

    #[inline]
    pub fn namespace_cache_disabled(&self) -> bool {
        self.root_namespace.cache_disabled.get()
    }

    /// Whether the normal module provider was exposed through globals()/locals().
    /// Explicit providers are cache-disabled from creation but retain their
    /// separate globals/locals semantics.
    pub fn namespace_globals_exposed(&self) -> bool {
        self.root_namespace.cache_disabled.get() && !self.namespace_is_explicit()
    }

    /// Whether controlled module writes must keep the root globals dictionary
    /// synchronized. Filesystem modules need this before exposure because their
    /// `PyModule` attribute surface reads the same backing; ordinary scripts
    /// need it only after globals have escaped.
    pub fn namespace_globals_require_mirroring(&self) -> bool {
        self.root_namespace.filesystem_module_backing.get() || self.namespace_globals_exposed()
    }
}
