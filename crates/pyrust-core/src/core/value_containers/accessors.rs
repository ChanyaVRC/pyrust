impl Value {
    /// Borrow the list's elements while holding the `RefCell` read guard.
    ///
    /// The guard prevents an aliased `Value` from mutating the same backing
    /// storage while the returned view is live.  Callers that may invoke
    /// Python code should copy the required values and drop this guard first.
    #[inline(always)]
    pub fn as_list(&self) -> Option<std::cell::Ref<'_, Vec<Value>>> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_list() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if self.is_list() {
            let inner = unsafe { self.list_inner() };
            Some(inner.items.borrow())
        } else {
            None
        }
    }

    // `as_list_mut` was removed in #448 — use the scoped operation
    // methods (`Value::list_with_mut`, `list_push`, `list_extend`, …)
    // instead.  The previous `unsafe { &mut *cell.as_ptr() }` pattern
    // exposed an unguarded `&mut Vec<Value>` across crate boundaries
    // and forced callers to manually re-derive the aliasing-safety
    // property that `RefCell` already enforces internally.

    /// Borrow the tuple's elements as a slice.  Backs both the heap path
    /// (`TAG_TUPLE`, an `Rc<TupleInner>` since #2268) and the inline small-tuple
    /// path (`Opaque::SmallTuple2/3`); see #281.
    pub fn as_tuple(&self) -> Option<&[Value]> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_tuple() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if top16(self.0) == TAG_TUPLE {
            return Some(&unsafe { self.tuple_inner() }.items);
        }
        if top16(self.0) == TAG_OPAQUE {
            match unsafe { &*self.opaque_ptr() } {
                Opaque::SmallTuple2 { items, .. } => return Some(&items[..]),
                Opaque::SmallTuple3 { items, .. } => return Some(&items[..]),
                _ => {}
            }
        }
        None
    }

    pub fn as_opaque(&self) -> Option<&Opaque> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_opaque() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if top16(self.0) == TAG_OPAQUE {
            Some(unsafe { &*self.opaque_ptr() })
        } else {
            None
        }
    }

    /// Zero-cost borrow of the inner `Rc<RefCell<PyInstance>>` without
    /// an `Rc::clone`.  Returns `None` for any non-PyInstance value.
    ///
    /// Used by the GetAttr / CallMethod inline cache to read the class
    /// pointer and version without paying the clone cost on the fast path.
    pub fn as_py_instance_rc(&self) -> Option<&Rc<RefCell<PyInstance>>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::PyInstance(rc) = o {
                Some(rc)
            } else {
                None
            }
        })
    }

    // `as_opaque_mut` removed in #448 — the only callers were the
    // `as_dict_mut` / `as_set_mut` accessors that have themselves
    // been retired.

    /// Borrow the dict while holding the `RefCell` read guard.
    #[inline(always)]
    pub fn as_dict(&self) -> Option<std::cell::Ref<'_, PyDict>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                Some(rc.borrow())
            } else {
                None
            }
        })
    }

    // `as_dict_mut` removed in #448 — use `Value::dict_with_mut`,
    // `dict_insert`, `dict_shift_remove`, `dict_clear`, `dict_extend`
    // instead.

    /// Borrow the set while holding the `RefCell` read guard.
    #[inline(always)]
    pub fn as_set(&self) -> Option<std::cell::Ref<'_, PySet>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Set(rc) = o {
                Some(rc.items.borrow())
            } else {
                None
            }
        })
    }

    // `as_set_mut` removed in #448 — use `Value::set_with_mut`,
    // `set_add`, `set_discard`, `set_clear`, `set_extend` instead.

    pub fn get_dict_rc(&self) -> Option<&Rc<RefCell<PyDict>>> {
        self.as_opaque().and_then(|o| {
            if let Opaque::Dict(rc) = o {
                Some(rc)
            } else {
                None
            }
        })
    }

    /// Register a live iterator against this dict's backing storage.
    pub fn dict_iteration_mutation_state(&self) -> Option<CollectionMutationState> {
        self.dict_rc().map(dict_iteration_mutation_state)
    }

    /// Register a live iterator against this set's backing storage.
    pub fn set_iteration_mutation_state(&self) -> Option<CollectionMutationState> {
        self.set_rc().map(|set| {
            collection_mutation_state(
                set_container_key(set),
                MutableContainerTarget::Set(Rc::downgrade(set)),
            )
        })
    }

    /// Borrow the list's `Rc<ListInner>`.  Returns `None` when `self`
    /// is not a list.  Internal helper for the operation methods.
    #[inline(always)]
    fn list_inner_rc(&self) -> Option<&ListInner> {
        if self.is_list() {
            Some(unsafe { self.list_inner() })
        } else {
            None
        }
    }

    /// Borrow the dict's `Rc<RefCell<IndexMap<...>>>`.
    #[inline(always)]
    fn dict_rc(&self) -> Option<&Rc<RefCell<PyDict>>> {
        self.get_dict_rc()
    }

    /// Borrow the set's shared backing.
    #[inline(always)]
    fn set_rc(&self) -> Option<&Rc<SetInner>> {
        self.as_opaque().and_then(|o| match o {
            Opaque::Set(rc) => Some(rc),
            _ => None,
        })
    }

    #[inline(always)]
    fn set_inner_rc(&self) -> Option<&SetInner> {
        self.set_rc().map(Rc::as_ref)
    }

    /// Length of the list.
    #[inline(always)]
    pub fn list_len(&self) -> Option<usize> {
        self.list_inner_rc().map(|i| i.items.borrow().len())
    }

    /// Length of the set.
    #[inline(always)]
    pub fn set_len(&self) -> Option<usize> {
        self.set_inner_rc().map(|i| i.items.borrow().len())
    }

    /// Length of the dict.
    #[inline(always)]
    pub fn dict_len(&self) -> Option<usize> {
        self.dict_rc().map(|rc| rc.borrow().len())
    }
}
