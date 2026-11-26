/// Cached locator for overwriting one existing root-namespace binding in
/// place (`StoreGlobal`, #3423).
///
/// The locator names the binding's [`EnvValues`] slot by index and key
/// identity, plus the one live Script register that mirrors the same name, if
/// any. It owns the key string only: neither the bound value, the register
/// file, nor its layout is retained. Every field is revalidated by
/// [`Environment::overwrite_namespace_store_slot`], so a stale locator is an
/// ordinary cache miss.
#[derive(Clone, Debug)]
pub struct NamespaceStoreSlot {
    namespace_id: u64,
    index: u32,
    key: Rc<str>,
    /// Mirror epoch the `mirror` decision was made under. An unchanged epoch
    /// proves that the same Script frames are live, so `None` still means "no
    /// live register mirrors this name".
    mirror_epoch: u64,
    mirror: Option<(Weak<HashMap<String, u32>>, u32)>,
}

/// What an in-place root binding overwrite displaced, and what remains to be
/// mirrored.
///
/// The displaced values are handed back so the caller drops them only after
/// releasing its borrow of the root environment.
#[must_use]
#[derive(Debug)]
pub struct NamespaceStoreOutcome {
    /// Previous value of the root binding.
    pub displaced: Value,
    /// Previous value of the mirroring Script register, if one was written.
    pub displaced_register: Option<Value>,
    /// Filesystem-module globals dictionary that must receive the same write.
    pub globals: Option<Value>,
}

impl RootNamespaceState {
    /// Resolve the register of this root's single live Script mirror, provided
    /// it is the mirror `layout` described and still covers `register`.
    fn single_mirror_register(
        &self,
        layout: &Weak<HashMap<String, u32>>,
        register: u32,
    ) -> Option<NonNull<Value>> {
        let active = self.active_mirrors.borrow();
        let [mirror] = active.as_slice() else {
            return None;
        };
        if !Weak::ptr_eq(&mirror.local_index, layout)
            || mirror.lifetime.strong_count() == 0
            || mirror.local_index.strong_count() == 0
            || register as usize >= mirror.regs_len
        {
            return None;
        }
        // SAFETY: the bounds check above keeps the offset inside the register
        // allocation the live mirror guard describes.
        Some(unsafe { mirror.regs_ptr.add(register as usize) })
    }
}

impl Environment {
    /// Locate `name` in the root namespace's binding storage and return a
    /// self-validating `(index, key)` locator, provided the stored value is the
    /// very object `expected` (#3423).
    ///
    /// Walks to the root from any lexical child. The identity check keeps a
    /// resolution that some other provider supplied from being attributed to
    /// this slot.
    pub fn namespace_root_binding_slot(
        &self,
        name: &str,
        expected: &Value,
    ) -> Option<(u32, Rc<str>)> {
        if let Some(parent) = &self.parent {
            return parent.borrow().namespace_root_binding_slot(name, expected);
        }
        let values = self.namespace_values();
        let (index, key) = values.slot_of(name)?;
        if !values.slot_value(index, key)?.is_identical_to(expected) {
            return None;
        }
        Some((u32::try_from(index).ok()?, Rc::clone(key)))
    }

    /// Read the root binding a `(index, key)` locator names, or `None` once
    /// that binding has been removed, re-inserted, or left unset.
    #[inline]
    pub fn namespace_root_slot_value(&self, index: u32, key: &Rc<str>) -> Option<Value> {
        if let Some(parent) = &self.parent {
            return Self::namespace_parent_slot_value(parent, index, key);
        }
        self.namespace_values()
            .slot_value(index as usize, key)
            .filter(|value| !value.is_unset())
            .cloned()
    }

    /// Out-of-line parent walk: only a function defined inside a class body or
    /// a PEP 695 scope reads a root binding from a lexical child.
    #[cold]
    #[inline(never)]
    fn namespace_parent_slot_value(parent: &EnvRef, index: u32, key: &Rc<str>) -> Option<Value> {
        parent.borrow().namespace_root_slot_value(index, key)
    }

    /// Record a precomputed name mask for a binding known to live in this
    /// root's `EnvValues`, so module fast-local writes keep it synchronized.
    #[inline]
    pub fn record_namespace_env_binding_mask(&self, name_mask: u64) {
        if self.root_namespace.env_binding_interest.get() & name_mask != name_mask {
            self.root_namespace
                .env_binding_interest
                .set(self.root_namespace.env_binding_interest.get() | name_mask);
            self.root_namespace.recompute_sync_fast_check_mask();
        }
    }

    /// Whether some `StoreGlobal` cache may overwrite a name with this mask in
    /// place, without advancing a cache generation.
    ///
    /// A `LoadGlobal` cache for such a name must be a slot locator: a cached
    /// value would never be invalidated by those overwrites.
    #[inline]
    pub fn namespace_store_slot_interested(&self, name_mask: u64) -> bool {
        self.root_namespace.store_slot_interest.get() & name_mask == name_mask
    }

    /// Build the in-place overwrite locator for `name`, which must already be
    /// bound in this root environment (#3423).
    ///
    /// Building a locator changes nothing: a name that is assigned once and
    /// never again (a module-level `def`, a constant) keeps its readers on the
    /// cheaper value lane. Only the first overwrite through the locator moves
    /// the name to the locator lane.
    ///
    /// Returns `None` whenever a later overwrite could not be proven
    /// equivalent to the full assignment path: an exposed or explicit
    /// namespace (whose dictionary is authoritative), a saturated identity or
    /// mirror epoch, or more than one live Script mirror.
    pub fn namespace_store_slot(&self, name: &str) -> Option<NamespaceStoreSlot> {
        let root = &*self.root_namespace;
        if self.parent.is_some() || root.id == u64::MAX || root.cache_disabled.get() {
            return None;
        }
        let mirror_epoch = root.fastlocal_mirror_epoch.get();
        if mirror_epoch == u64::MAX {
            return None;
        }
        let (index, key) = self.namespace_values().slot_of(name)?;
        let mirror = {
            let active = root.active_mirrors.borrow();
            match active.as_slice() {
                [] => None,
                [mirror] => {
                    if mirror.lifetime.strong_count() == 0 {
                        return None;
                    }
                    let layout = mirror.local_index.upgrade()?;
                    match layout.get(name) {
                        Some(&register) if (register as usize) < mirror.regs_len => {
                            Some((mirror.local_index.clone(), register))
                        }
                        Some(_) => return None,
                        None => None,
                    }
                }
                _ => return None,
            }
        };
        Some(NamespaceStoreSlot {
            namespace_id: root.id,
            index: u32::try_from(index).ok()?,
            key: Rc::clone(key),
            mirror_epoch,
            mirror,
        })
    }

    /// Overwrite the existing root binding `slot` names with `value`,
    /// reproducing every effect of a full global assignment to an
    /// already-bound name (#3423).
    ///
    /// Returns `None`, having changed nothing, when any guard fails: a
    /// different or exposed namespace, a changed set of live Script frames, or
    /// a binding that was removed or re-inserted since the locator was built.
    ///
    /// A repeated overwrite advances no cache generation. The first overwrite
    /// of a name registers it in the root's store-slot interest and advances
    /// the generations once, exactly as a full assignment would: that
    /// invalidates every value-backed `LoadGlobal` entry for the name, and the
    /// registration keeps any later entry on the locator lane, which reads the
    /// live binding. From then on no cache can hold a value this overwrite
    /// replaces. `name_mask` must be `namespace_name_interest_mask(name)`.
    #[inline]
    pub fn overwrite_namespace_store_slot(
        &mut self,
        slot: &NamespaceStoreSlot,
        name: &str,
        name_mask: u64,
        value: &Value,
    ) -> Option<NamespaceStoreOutcome> {
        debug_assert_eq!(name_mask, namespace_name_interest_mask(name));
        let root = &*self.root_namespace;
        if root.id != slot.namespace_id
            || root.cache_disabled.get()
            || root.fastlocal_mirror_epoch.get() != slot.mirror_epoch
        {
            return None;
        }
        // Resolve the mirror register before mutating anything, so a miss
        // leaves the namespace untouched for the full assignment path.
        let register = match &slot.mirror {
            Some((layout, register)) => Some(root.single_mirror_register(layout, *register)?),
            None => None,
        };
        let EnvironmentStorage::Namespace(values) = &mut self.storage;
        let binding = values.slot_value_mut(slot.index as usize, &slot.key)?;
        let displaced = std::mem::replace(binding, value.clone());

        let root = &*self.root_namespace;
        if root.store_slot_interest.get() & name_mask != name_mask {
            // The interest set is a Bloom mask, so *every* change to it must
            // advance the generations, not only the first overwrite of a
            // given name: a name may later find its bits already set by
            // others and skip this branch, which is sound only because the
            // change that completed its coverage invalidated every
            // value-backed entry, and no such entry is filled once covered.
            root.store_slot_interest
                .set(root.store_slot_interest.get() | name_mask);
            root.bump_structure_version();
        }
        if root.has_binding_order.get() {
            self.record_namespace_binding_order_insert(name);
        }
        self.record_namespace_env_binding_mask(name_mask);
        // SAFETY: `single_mirror_register` proved the mirror guard is live and
        // the register in bounds, and nothing since could have ended that
        // Script frame. Script execution accesses its registers through raw
        // pointers only, so no `&mut [Value]` aliases this slot.
        let displaced_register =
            register.map(|mut slot| std::mem::replace(unsafe { slot.as_mut() }, value.clone()));
        let globals = if root.filesystem_module_backing.get() {
            self.bump_filesystem_module_mutation();
            Some(root.globals_provider_snapshot().0)
        } else {
            None
        };
        Some(NamespaceStoreOutcome {
            displaced,
            displaced_register,
            globals,
        })
    }
}
