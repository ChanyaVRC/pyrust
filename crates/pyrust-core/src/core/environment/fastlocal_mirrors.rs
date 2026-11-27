impl Environment {
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
        self.root_namespace.recompute_sync_fast_check_mask();
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
    ///
    /// Within one mirror the slots are visited in register order. The compiler
    /// allocates a script's registers from `collect_local_names`, which walks
    /// the module body in source order, so register order *is* the module's
    /// binding order — the order CPython's module dict preserves (issue
    /// #2903). The result keeps each name at the position of its first
    /// binding while taking its value from the innermost mirror that bound it.
    pub fn namespace_fastlocals_snapshot(&self) -> Vec<(String, Value)> {
        self.declared_bindings_in_binding_order(false)
            .into_iter()
            .collect()
    }

    /// Shared slot walk behind the two namespace snapshots.
    ///
    /// `env_fallback` additionally resolves a declared name whose register is
    /// unset from this environment's own bindings, keeping it at its declared
    /// position instead of appending it later.
    fn declared_bindings_in_binding_order(&self, env_fallback: bool) -> IndexMap<String, Value> {
        let mut declared: IndexMap<String, Value> = IndexMap::new();
        for mirror in self.root_namespace.live_mirrors() {
            let mut by_slot: Vec<(u32, &String)> = mirror
                .local_index
                .iter()
                .map(|(name, &slot)| (slot, name))
                .filter(|(slot, _)| (*slot as usize) < mirror.regs_len)
                .collect();
            // The name tie-breaker only matters for a layout that maps two
            // names onto one register; it keeps the result independent of the
            // layout map's iteration order in every case.
            by_slot.sort_unstable();
            for (slot, name) in by_slot {
                // SAFETY: `live_mirrors` retains the mirror guard lifetime and
                // the slot was bounds checked by the filter above.
                let value = unsafe { mirror.regs_ptr.add(slot as usize).as_ref() };
                if !value.is_unset() {
                    declared.insert(name.clone(), value.clone());
                } else if env_fallback
                    && !declared.contains_key(name.as_str())
                    && let Some(value) = self.namespace_values().get(name)
                {
                    declared.insert(name.clone(), value.clone());
                }
            }
        }
        declared
    }

    /// Ordered `(name, value)` pairs that materialise this root's Python-visible
    /// namespace dictionary (issue #2903).
    ///
    /// A module namespace is a Python dict, so its iteration order is part of
    /// the language semantics. This walk reproduces the module's binding order:
    ///
    /// 1. bindings that only exist in `EnvValues` — the pre-seeded module
    ///    dunders, names created by a `global` write from a function, earlier
    ///    REPL statements — in their own insertion order;
    /// 2. every name a live Script layout declares, in register order, which is
    ///    the compiler's source binding order. A declared name whose register
    ///    is unset because its value lives in `EnvValues` (a module-level cell
    ///    variable captured by a nested function) keeps its declared position.
    ///
    /// Names already present in the target dictionary keep the position they
    /// were first inserted at, exactly like `dict.__setitem__`.
    pub fn namespace_materialization_snapshot(&self) -> Vec<(String, Value)> {
        if self.root_namespace.has_binding_order.get()
            && let Some(order) = self.root_namespace.binding_order.borrow().as_deref()
        {
            let mut live: IndexMap<String, Value> = self
                .namespace_values()
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect();
            for (name, value) in self.declared_bindings_in_binding_order(false) {
                live.insert(name, value);
            }
            let mut ordered = Vec::with_capacity(live.len());
            for name in order.iter() {
                if let Some(value) = live.shift_remove(name.as_str()) {
                    ordered.push((name.clone(), value));
                }
            }
            ordered.extend(live);
            return ordered;
        }
        let declared = self.declared_bindings_in_binding_order(true);
        let mut ordered: Vec<(String, Value)> = self
            .namespace_values()
            .iter()
            .filter(|(name, _)| !declared.contains_key(*name))
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect();
        ordered.extend(declared);
        ordered
    }

    /// Whether this root namespace has activated its lazy binding-order
    /// journal (issue #2919).
    #[inline]
    pub fn namespace_has_binding_order(&self) -> bool {
        self.root_namespace.has_binding_order.get()
    }

    /// Whether at least one Script fast-local mirror is currently active on
    /// this root namespace.
    #[inline]
    pub fn namespace_has_active_fastlocal_mirror(&self) -> bool {
        self.root_namespace.active_mirror_count.get() > 0
    }

    /// Whether any live Script fast-local mirror declares a slot for `name`.
    pub fn namespace_fastlocal_declares(&self, name: &str) -> bool {
        self.root_namespace
            .live_mirrors()
            .iter()
            .any(|mirror| mirror.local_index.contains_key(name))
    }

    fn ensure_binding_order_initialized(&self) {
        if self.root_namespace.has_binding_order.get() {
            return;
        }
        let initial: IndexSet<String, FxBuildHasher> = self
            .namespace_materialization_snapshot()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        *self.root_namespace.binding_order.borrow_mut() = Some(Box::new(initial));
        self.root_namespace.has_binding_order.set(true);
        self.root_namespace.recompute_sync_fast_check_mask();
    }

    /// Record a binding insertion when the binding-order journal is active.
    /// Re-binding a name that is already present keeps its existing position.
    #[inline]
    pub fn record_namespace_binding_order_insert(&self, name: &str) {
        if !self.root_namespace.has_binding_order.get() {
            return;
        }
        if let Some(order) = self
            .root_namespace
            .binding_order
            .borrow_mut()
            .as_deref_mut()
            && !order.contains(name)
        {
            order.insert(name.to_string());
        }
    }

    /// Activate the binding-order journal before an out-of-slot global
    /// insertion (such as a `global`-write-only name or `from mod import *`)
    /// so the newly bound key lands at its runtime binding point rather than
    /// before declared fast-local slots.
    pub fn record_namespace_out_of_order_insert(&self, name: &str) {
        if self.root_namespace.mapping_authoritative() {
            return;
        }
        self.ensure_binding_order_initialized();
        if let Some(order) = self
            .root_namespace
            .binding_order
            .borrow_mut()
            .as_deref_mut()
            && !order.contains(name)
        {
            order.insert(name.to_string());
        }
    }

    /// Activate the binding-order journal before deleting a module binding so
    /// a later re-bind of the same name appends at the end instead of reusing
    /// its static register position.
    pub fn record_namespace_binding_order_delete(&self, name: &str) {
        if self.root_namespace.mapping_authoritative() {
            return;
        }
        self.ensure_binding_order_initialized();
        if let Some(order) = self
            .root_namespace
            .binding_order
            .borrow_mut()
            .as_deref_mut()
        {
            order.shift_remove(name);
        }
    }

    /// Snapshot the recorded binding order for post-frame register write-back.
    pub fn namespace_binding_order_snapshot(&self) -> Vec<String> {
        self.root_namespace
            .binding_order
            .borrow()
            .as_deref()
            .map(|order| order.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Reorder the namespace bindings to match the recorded binding-order journal
    /// after fast-local write-back completes.
    pub fn reorder_values_to_binding_order(&mut self) {
        if !self.root_namespace.has_binding_order.get() {
            return;
        }
        let order = self.namespace_binding_order_snapshot();
        if order.is_empty() {
            return;
        }
        let values = self.namespace_values_mut();
        let mut remaining: IndexMap<String, Value> = values
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()))
            .collect();
        values.clear();
        for name in &order {
            if let Some(value) = remaining.shift_remove(name.as_str()) {
                values.insert(name, value);
            }
        }
        for (name, value) in remaining {
            values.insert(&name, value);
        }
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
}
