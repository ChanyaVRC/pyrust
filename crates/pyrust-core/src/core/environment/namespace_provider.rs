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
        let mut flags = 0u8;
        if filesystem_module_backing || self.root_namespace.mapping_authoritative() {
            flags |= Self::MODULE_WRITE_SYNC_GLOBALS;
        }
        // A normal root propagates its own writes to sibling Script mirrors,
        // exposed or not: its mirroring dictionary write is an owner write,
        // which the storage notification skips. Only an explicit provider
        // leaves this to the notification.
        if !self.root_namespace.cache_disabled.get()
            && self.root_namespace.active_mirror_count.get() > 1
        {
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
        if !filesystem_module_backing && !self.root_namespace.mapping_authoritative() {
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
    /// Explicit providers are permanently uncached: their dictionaries are the
    /// whole namespace, so every lookup reads them directly.
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
    ///
    /// Exposure keeps the `LoadGlobal` caches enabled (#3425). The first
    /// exposure advances the structure generation once, so every entry that
    /// survives was filled while the dictionary was already authoritative;
    /// from then on each mutation through any alias of the dictionary advances
    /// that generation again at the storage boundary.
    pub fn expose_namespace_globals(&self) -> Value {
        let (globals, _, explicit) = self.root_namespace.globals_provider_snapshot();
        if !explicit && !self.root_namespace.globals_exposed.replace(true) {
            self.root_namespace.bump_structure_version();
        }
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

    /// Write one binding this root has just changed into `mapping`, the
    /// dictionary that mirrors it, and return the value it displaced.
    ///
    /// For a normal provider this is an *owner write*: the storage
    /// notification skips this root, because the caller has already advanced
    /// the generations its cache-interest masks require and keeps its own
    /// Script mirrors current. That is what lets an exposed namespace keep its
    /// `LoadGlobal` caches across its own assignments (#3425). Every other
    /// root that aliases `mapping` is still notified, and an explicit provider
    /// is notified as usual.
    ///
    /// An existing key is replaced in place without allocating an owned key.
    /// The displaced value is returned so the caller drops it only after
    /// releasing its borrow of this environment.
    ///
    /// `mapping` must be the dictionary this root mirrors into: a normal
    /// provider's own globals, or an explicit provider's globals or locals.
    pub fn write_namespace_mapping_binding(
        &self,
        mapping: &Value,
        name: &str,
        value: Value,
    ) -> Option<Value> {
        let root = &*self.root_namespace;
        let owner = !root.cache_disabled.get();
        // A normal provider whose mapping no other root aliases has nobody to
        // notify about an in-place replacement.
        let reported_name = (!owner || root.alias_shared.get()).then_some(name);
        let previous = root.owner_write.replace(owner);
        let displaced =
            match mapping.dict_replace_existing(&StrKey(name), true, reported_name, value) {
                Ok(Ok(displaced)) => Some(displaced),
                Ok(Err(value)) => mapping
                    .dict_insert(crate::object_model::PyKey::str_from(name), value)
                    .ok()
                    .flatten(),
                Err(_) => None,
            };
        root.owner_write.set(previous);
        displaced
    }

    /// Whether this root is an explicit `exec`/`eval` provider, for which no
    /// `LoadGlobal` cache entry is ever filled or probed. Exposing a normal
    /// provider's globals mapping does not disable its caches (#3425).
    #[inline]
    pub fn namespace_cache_disabled(&self) -> bool {
        self.root_namespace.cache_disabled.get()
    }

    /// Whether a Python-visible dictionary is the authoritative namespace: an
    /// explicit provider, or a normal provider whose globals mapping escaped.
    /// Lookups must then refresh a binding from that dictionary before
    /// trusting `EnvValues`, and controlled writes must mirror into it.
    #[inline]
    pub fn namespace_mapping_authoritative(&self) -> bool {
        self.root_namespace.mapping_authoritative()
    }

    /// Whether the normal module provider was exposed through globals()/locals().
    /// Explicit providers are cache-disabled from creation but retain their
    /// separate globals/locals semantics.
    #[inline]
    pub fn namespace_globals_exposed(&self) -> bool {
        self.root_namespace.globals_exposed.get()
    }

    /// Whether controlled module writes must keep the root globals dictionary
    /// synchronized. Filesystem modules need this before exposure because their
    /// `PyModule` attribute surface reads the same backing; ordinary scripts
    /// need it only after globals have escaped.
    pub fn namespace_globals_require_mirroring(&self) -> bool {
        self.root_namespace.filesystem_module_backing.get() || self.namespace_globals_exposed()
    }
}
