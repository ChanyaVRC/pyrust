/// A single lexical scope's binding storage.
///
/// `Environment` is the **slow, name-keyed** half of pyrust's dual storage:
/// `values` holds module/class-body bindings, closure/`nonlocal` cells, and
/// any function local the compiler chose not to register-allocate. The
/// **fast, index-keyed** half (fastlocals) is not stored here — it lives in
/// the active VM frame's register file, keyed by compile-time slot analysis.
///
/// `values` is an [`EnvValues`]: an inline small-vector for the common
/// closure/generator capture (1–2 cells, zero heap allocation) that promotes to
/// a hashed map for module/class scope (issue #452). This is the closure-memory
/// stage of #452; the full opcode-level collapse (LoadFast/LoadCell/LoadName)
/// remains future work.
///
/// Which store a name uses, and which lookup/assign path handles it (keyed by
/// `global_names` / `nonlocal_names` / `local_names` and whether `parent` is
/// `None`), is decided at compile time. The single authoritative description
/// of that rule — "THE RULE" — lives next to the runtime helpers in
/// `crates/pyrust/src/interpreter/helpers.rs` (search for
/// "the env-lookup rule (issue #452)"). Keep these in sync.
#[derive(Debug, Clone)]
pub struct Environment {
    /// Identity, mutation generations, and globals provider shared by every
    /// lexical scope below the same root namespace.
    root_namespace: Rc<RootNamespaceState>,
    pub values: EnvValues,
    pub local_names: NameSet,
    pub global_names: NameSet,
    pub nonlocal_names: NameSet,
    pub parent: Option<EnvRef>,
}

pub type EnvRef = Rc<RefCell<Environment>>;

impl Environment {
    #[inline]
    fn root_namespace_for_parent(parent: Option<&EnvRef>) -> Rc<RootNamespaceState> {
        parent
            .map(|parent| Rc::clone(&parent.borrow().root_namespace))
            .unwrap_or_else(|| Rc::new(RootNamespaceState::new()))
    }

    pub fn new(parent: Option<EnvRef>) -> EnvRef {
        let root_namespace = Self::root_namespace_for_parent(parent.as_ref());
        Rc::new(RefCell::new(Self {
            root_namespace,
            values: EnvValues::new(),
            local_names: Rc::new(HashSet::new()),
            global_names: Rc::new(HashSet::new()),
            nonlocal_names: Rc::new(HashSet::new()),
            parent,
        }))
    }

    /// Reset a uniquely-owned environment before returning it from the runtime
    /// pool.  Inheriting the parent's shared state is essential: a pooled frame
    /// may previously have belonged to a different module namespace.
    pub fn reset_for_reuse(&mut self, parent: Option<EnvRef>) {
        self.root_namespace = Self::root_namespace_for_parent(parent.as_ref());
        self.values.clear();
        self.parent = parent;
        self.local_names = Rc::new(HashSet::new());
        self.global_names = Rc::new(HashSet::new());
        self.nonlocal_names = Rc::new(HashSet::new());
    }

    /// Stable identity of the root namespace. `u64::MAX` is an uncacheable
    /// saturation sentinel.
    #[inline]
    pub fn namespace_id(&self) -> u64 {
        self.root_namespace.id
    }

    /// Return the complete inline-cache key state with one environment borrow.
    #[inline]
    pub fn namespace_cache_snapshot(&self) -> (u64, u64, u64, u64, bool) {
        (
            self.root_namespace.id,
            self.root_namespace.environment_version.get(),
            self.root_namespace.structure_version.get(),
            self.root_namespace.fastlocal_mirror_epoch.get(),
            self.root_namespace.cache_disabled.get(),
        )
    }

    #[inline]
    pub fn bump_namespace_environment_version(&self) {
        self.root_namespace.bump_environment_version();
    }

    /// Register a precomputed name mask for an environment-backed value that
    /// is about to be published in a `LoadGlobal` cache.
    #[inline]
    pub fn register_namespace_value_cache_interest(&self, name_mask: u64) {
        self.root_namespace
            .value_cache_interest
            .set(self.root_namespace.value_cache_interest.get() | name_mask);
    }

    /// Register a precomputed name mask for a canonical fallback value that is
    /// about to be published in a `LoadGlobal` cache.
    #[inline]
    pub fn register_namespace_fallback_cache_interest(&self, name_mask: u64) {
        self.root_namespace
            .fallback_cache_interest
            .set(self.root_namespace.fallback_cache_interest.get() | name_mask);
    }

    #[inline]
    pub fn record_namespace_env_binding(&self, name: &str) {
        self.root_namespace.env_binding_interest.set(
            self.root_namespace.env_binding_interest.get() | namespace_name_interest_mask(name),
        );
    }

    #[inline]
    pub fn namespace_env_binding_may_overlap(&self, name_mask: u64) -> bool {
        self.root_namespace.env_binding_interest.get() & name_mask == name_mask
    }

    /// Register one live Script fast-local register file for synchronization
    /// with mutations of this root's exposed namespace dictionary.
    ///
    /// # Safety
    ///
    /// `regs_ptr` must remain valid for `regs_len` consecutive [`Value`] slots
    /// until the returned guard is dropped. The pointed-to register file must
    /// use raw-pointer/[`crate::Value`] access rather than coexist with a live
    /// mutable slice while notifications may run.
    pub unsafe fn register_namespace_fastlocals(
        &self,
        regs_ptr: NonNull<Value>,
        regs_len: usize,
        local_index: &Rc<HashMap<String, u32>>,
    ) -> NamespaceMirrorGuard {
        let mut id = self.root_namespace.next_mirror_id.get();
        loop {
            if id != 0
                && !self
                    .root_namespace
                    .active_mirrors
                    .borrow()
                    .iter()
                    .any(|mirror| mirror.id == id)
            {
                break;
            }
            id = id.wrapping_add(1);
        }
        self.root_namespace.next_mirror_id.set(id.wrapping_add(1));
        let lifetime = Rc::new(());
        self.root_namespace
            .active_mirrors
            .borrow_mut()
            .push(ActiveNamespaceMirror {
                id,
                regs_ptr,
                regs_len,
                local_index: Rc::downgrade(local_index),
                lifetime: Rc::downgrade(&lifetime),
            });
        self.root_namespace.active_mirror_count.set(
            self.root_namespace
                .active_mirror_count
                .get()
                .saturating_add(1),
        );
        self.root_namespace.fastlocal_mirror_epoch.set(
            self.root_namespace
                .fastlocal_mirror_epoch
                .get()
                .saturating_add(1),
        );
        NamespaceMirrorGuard {
            root_namespace: Rc::downgrade(&self.root_namespace),
            id,
            _lifetime: lifetime,
        }
    }

    /// Snapshot every live Script fast-local mirror for this root.
    ///
    /// Mirrors are visited in registration order so a nested Script frame is
    /// authoritative over an outer frame for names it has already bound.
    /// Unset inner slots do not erase an outer/root binding.
    pub fn namespace_fastlocals_snapshot(&self) -> Vec<(String, Value)> {
        let mut snapshot = HashMap::new();
        for mirror in self.root_namespace.live_mirrors() {
            for (name, &slot) in mirror.local_index.iter() {
                let slot = slot as usize;
                if slot >= mirror.regs_len {
                    continue;
                }
                // SAFETY: `live_mirrors` retains the mirror guard lifetime and
                // the slot has been bounds checked.
                let value = unsafe { mirror.regs_ptr.add(slot).as_ref() };
                if !value.is_unset() {
                    snapshot.insert(name.clone(), value.clone());
                }
            }
        }
        snapshot.into_iter().collect()
    }

    /// Read the innermost live Script fast-local binding for one name.
    pub fn namespace_fastlocal_value(&self, name: &str) -> Option<Value> {
        for mirror in self.root_namespace.live_mirrors().into_iter().rev() {
            let Some(&slot) = mirror.local_index.get(name) else {
                continue;
            };
            let slot = slot as usize;
            if slot >= mirror.regs_len {
                continue;
            }
            // SAFETY: `live_mirrors` retains the mirror guard lifetime and the
            // slot has been bounds checked.
            let value = unsafe { mirror.regs_ptr.add(slot).as_ref() };
            if !value.is_unset() {
                return Some(value.clone());
            }
        }
        None
    }

    /// Resolve one live fastlocal together with a pointer-free cache locator.
    ///
    /// The weak layout identity and register number are valid only while the
    /// returned mirror epoch matches. Inline caches must validate them through
    /// [`Self::namespace_fastlocal_cached_value`].
    pub fn namespace_fastlocal_cache_source(
        &self,
        name: &str,
    ) -> Option<NamespaceFastLocalCacheSource> {
        let mirror_epoch = self.root_namespace.fastlocal_mirror_epoch.get();
        if mirror_epoch == u64::MAX {
            return None;
        }
        let active = self.root_namespace.active_mirrors.borrow();
        for mirror in active.iter().rev() {
            if mirror.lifetime.strong_count() == 0 {
                continue;
            }
            let Some(local_index) = mirror.local_index.upgrade() else {
                continue;
            };
            let Some(&register) = local_index.get(name) else {
                continue;
            };
            let slot = register as usize;
            if slot >= mirror.regs_len {
                continue;
            }
            // SAFETY: the guard lifetime proves the register allocation is
            // live, and the slot has been bounds checked.
            let value = unsafe { mirror.regs_ptr.add(slot).as_ref() };
            if !value.is_unset() {
                return Some(NamespaceFastLocalCacheSource {
                    value: value.clone(),
                    local_index: Rc::downgrade(&local_index),
                    register,
                    mirror_epoch,
                });
            }
        }
        None
    }

    /// Validate and read a cached Script fastlocal locator.
    ///
    /// No raw register pointer leaves this root. Layout identity, mirror
    /// lifetime, epoch, and bounds are checked before the one cloned read.
    pub fn namespace_fastlocal_cached_value(
        &self,
        expected_mirror_epoch: u64,
        expected_local_index: &Weak<HashMap<String, u32>>,
        register: u32,
    ) -> Option<Value> {
        if expected_mirror_epoch == u64::MAX
            || self.root_namespace.fastlocal_mirror_epoch.get() != expected_mirror_epoch
        {
            return None;
        }
        let active = self.root_namespace.active_mirrors.borrow();
        for mirror in active.iter().rev() {
            if !Weak::ptr_eq(&mirror.local_index, expected_local_index) {
                continue;
            }
            let Some(_lifetime) = mirror.lifetime.upgrade() else {
                continue;
            };
            let Some(_local_index) = mirror.local_index.upgrade() else {
                continue;
            };
            let slot = register as usize;
            if slot >= mirror.regs_len {
                continue;
            }
            // SAFETY: the matching guard lifetime proves the register
            // allocation is live, and the slot has been bounds checked.
            let value = unsafe { mirror.regs_ptr.add(slot).as_ref() };
            if !value.is_unset() {
                return Some(value.clone());
            }
        }
        None
    }

    /// Propagate one controlled module binding change to every live Script
    /// mirror except the register file that originated the write.
    ///
    /// This is required before the globals dictionary escapes: there is no
    /// storage-level alias callback yet, but nested Script frames and global
    /// writes from functions must still observe one module namespace.
    pub fn synchronize_namespace_fastlocal_binding(
        &self,
        name: &str,
        value: &Value,
        writer: Option<NonNull<Value>>,
    ) {
        for mirror in self.root_namespace.live_mirrors() {
            if writer.is_some_and(|writer| writer == mirror.regs_ptr) {
                continue;
            }
            let Some(&slot) = mirror.local_index.get(name) else {
                continue;
            };
            let slot = slot as usize;
            if slot >= mirror.regs_len {
                continue;
            }
            // SAFETY: `live_mirrors` retains the mirror guard lifetime and the
            // slot has been bounds checked.
            unsafe {
                *mirror.regs_ptr.add(slot).as_mut() = value.clone();
            }
        }
    }

    /// Refresh all live Script mirrors from this root's authoritative mapping.
    /// Used by whole-namespace replacement paths that cannot describe their
    /// mutation as one string key.
    pub fn synchronize_namespace_fastlocals_from_mapping(&self, mapping: &Value) {
        if let Some(dict) = mapping.get_dict_rc() {
            self.root_namespace.synchronize_alias_full_mutation(dict);
        }
    }

    /// Whether a module fast-local write has another live Script mirror.
    ///
    /// The common one-frame script path uses this check to avoid name-pool
    /// lookup and cloning solely for mirror propagation.
    #[inline]
    pub fn namespace_has_sibling_fastlocal_mirror(&self) -> bool {
        self.root_namespace.active_mirror_count.get() > 1
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
    }

    /// Link a filesystem `PyModule` to this root namespace.
    ///
    /// The module keeps only a weak environment link, so circular imports do
    /// not add a strong `PyModule -> Environment -> PyModule` cycle. Functions
    /// retain their captured environment independently, while the module owns
    /// the shared globals dictionary for constant-only modules.
    pub fn configure_filesystem_module_namespace(&self, module_mutation: ModuleMutationState) {
        self.root_namespace.filesystem_module_backing.set(true);
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
