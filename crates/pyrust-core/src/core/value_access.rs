impl Value {
    // ── Type checks ──────────────────────────────────────────────────────────

    pub fn is_none(&self) -> bool {
        self.0 == TAG_NONE_BITS
    }

    pub fn is_bool(&self) -> bool {
        top16(self.0) == TAG_BOOL
    }

    pub fn is_int(&self) -> bool {
        top16(self.0) == TAG_INT
            || (top16(self.0) == TAG_OPAQUE
                && matches!(unsafe { &*self.opaque_ptr() }, Opaque::PyBigInt(_)))
    }

    pub fn is_float(&self) -> bool {
        // Exclude the reserved positive-NaN sentinels (UNSET, NotImplemented,
        // Ellipsis) whose `top16` falls in the float range but which aren't floats.
        if self.0 == NOT_IMPLEMENTED_BITS || self.0 == UNSET_BITS || self.0 == ELLIPSIS_BITS {
            return false;
        }
        top16(self.0) <= TAG_FLOAT_MAX
    }

    pub fn is_str(&self) -> bool {
        top16(self.0) == TAG_STR
    }

    pub fn is_tuple(&self) -> bool {
        if top16(self.0) == TAG_TUPLE {
            return true;
        }
        // Small tuples (2/3 elements) live in `Opaque::SmallTuple2/3` to
        // avoid the backing `Vec<Value>` heap allocation.  See #281.
        if top16(self.0) == TAG_OPAQUE {
            return matches!(
                unsafe { &*self.opaque_ptr() },
                Opaque::SmallTuple2 { .. } | Opaque::SmallTuple3 { .. }
            );
        }
        false
    }

    /// `true` only for a heap-backed tuple (`TAG_TUPLE`, an `Rc<TupleInner>`).
    /// Since #2268 a heap-tuple `clone` is an O(1) refcount bump like `list`, so
    /// this no longer marks the O(N)-clone outlier; it still gates the VM's
    /// by-move builtin-arg fast path (#2251), which is now a cheap micro-opt
    /// rather than an O(N) avoidance.  `SmallTuple2/3` (≤3 elements) return
    /// `false` — they are inline, not heap.
    #[inline]
    pub fn is_heap_tuple(&self) -> bool {
        top16(self.0) == TAG_TUPLE
    }

    pub fn is_list(&self) -> bool {
        top16(self.0) == TAG_LIST
    }

    /// Tag-only dict classification. Unlike `as_dict`, this does not borrow
    /// the dict's mutable backing storage.
    #[inline(always)]
    pub fn is_dict(&self) -> bool {
        top16(self.0) == TAG_OPAQUE && matches!(unsafe { &*self.opaque_ptr() }, Opaque::Dict(_))
    }

    /// Tag-only set classification. Unlike `as_set`, this does not borrow the
    /// set's mutable backing storage.
    #[inline(always)]
    pub fn is_set(&self) -> bool {
        top16(self.0) == TAG_OPAQUE && matches!(unsafe { &*self.opaque_ptr() }, Opaque::Set(_))
    }

    /// Return the shared function object carried by this value.
    ///
    /// Both Python functions and native builtins use
    /// `Opaque::UserFunction(Rc<UserFunction>)`; [`ValueKind::BuiltinFunction`]
    /// intentionally exposes only the registry dispatch name.  Identity,
    /// hashing, and cold attribute-state paths use this accessor when they need
    /// the actual per-object `Rc` without changing the name-based hot-call
    /// dispatch representation.
    #[inline]
    pub fn as_function_rc(&self) -> Option<&Rc<UserFunction>> {
        if top16(self.0) != TAG_OPAQUE {
            return None;
        }
        match unsafe { &*self.opaque_ptr() } {
            Opaque::UserFunction(function) => Some(function),
            _ => None,
        }
    }

    /// Fast, allocation-/borrow-free check used by membership-search hot loops
    /// (`list.remove` / `index` / `count`): returns `true` when this value
    /// definitely **cannot** fire a user `__eq__` and so can be compared with
    /// the primitive `Value::eq` fast path.
    ///
    /// Only the scalar NaN-box tags (`Float`/`None`/`Bool`/`Int`/`Str`) are
    /// recognised here — they classify from `top16` alone with no pointer
    /// deref.  Everything else (`List`/`Tuple`, and every `TAG_OPAQUE` payload,
    /// which may be a `PyInstance`/`Dict`/`Set`/small-tuple/`BuiltinObject`)
    /// conservatively returns `false`, deferring to the full `kind()`-based
    /// classification.  `BigInt` is opaque so it falls into the conservative
    /// arm — correct, just not the very fastest path for big-int searches.
    pub fn cannot_user_eq(&self) -> bool {
        if self.0 == NOT_IMPLEMENTED_BITS || self.0 == UNSET_BITS || self.0 == ELLIPSIS_BITS {
            return false;
        }
        matches!(
            top16(self.0),
            t if t <= TAG_FLOAT_MAX
        ) || matches!(top16(self.0), TAG_NONE | TAG_BOOL | TAG_INT | TAG_STR)
    }

    /// Identity short-circuit for sequence searches (`x in [x]`, `.index`,
    /// `.count`, list/tuple `==`).  CPython's `PyObject_RichCompareBool` treats
    /// `a is b` as equal *before* calling `__eq__`, which is observable only for
    /// `NaN` — the one primitive where `x == x` is `False`.
    ///
    /// pyrust's floats are NaN-boxed immediates, not heap objects, so object
    /// identity is carried in the NaN payload instead: `Value::float` mints a
    /// fresh payload for every NaN it boxes, which makes raw bit-pattern
    /// equality an *exact* identity test rather than an approximation (#2911).
    /// Two NaNs are reported as the same object here precisely when they came
    /// from the same boxing, matching CPython's `a is b`.  Restricted to floats
    /// and complex (whose components are also bit-copied f64s) so no other
    /// type's equality semantics change.
    #[inline]
    pub fn is_identical_nan(&self, other: &Self) -> bool {
        if self.0 == other.0 {
            // Same NaN-boxed bits: the float arm (the original #2344 case).
            if self.is_float() {
                return f64::from_bits(self.0).is_nan();
            }
            // Opaque clones now share one refcounted slot, so two aliases have
            // identical NaN-box bits.  Preserve the complex-NaN identity
            // short-circuit that previously ran only after wrapper reallocation.
            if top16(self.0) == TAG_OPAQUE {
                return self.opaque_identical_nan(other);
            }
            return false;
        }
        // Distinct bits.  The only non-identical pair that can still be "the same
        // object" for RichCompareBool is a NaN-bearing complex (two heap allocs).
        // Keep this wrapper tiny so it inlines into `try_seq_fast_eq` /
        // membership loops; the scalar callers (int/float/str) bail on the single
        // `top16` tag check and never pay the pointer-deref (#2535 perf).
        if top16(self.0) == TAG_OPAQUE && top16(other.0) == TAG_OPAQUE {
            return self.opaque_identical_nan(other);
        }
        false
    }

    /// Cold tail of [`Value::is_identical_nan`] for two `TAG_OPAQUE` operands.
    /// Identity can't be the raw `self.0 == other.0` pointer compare the float
    /// arm uses — two distinct heap allocations of the same complex are still
    /// "the same value".  Mirror the float intent: when a component is NaN,
    /// treat bit-identical components as the same object so a freshly-inserted
    /// NaN-bearing complex stays findable (`z = complex(nan, 0); z in [z]`).
    #[cold]
    #[inline(never)]
    fn opaque_identical_nan(&self, other: &Self) -> bool {
        if let (Opaque::Complex(ar, ai), Opaque::Complex(br, bi)) =
            (unsafe { &*self.opaque_ptr() }, unsafe {
                &*other.opaque_ptr()
            })
            && (ar.is_nan() || ai.is_nan())
        {
            return ar.to_bits() == br.to_bits() && ai.to_bits() == bi.to_bits();
        }
        false
    }

    /// Returns a stable identity value for pool-allocated and Rc-shared types:
    /// - tuple: reads `obj_id` from the shared [`TupleInner`]; aliased clones
    ///   (Rc-shared) all surface the same id, matching `b = a` aliasing (#2268)
    /// - list: reads `obj_id` from the shared [`ListInner`]; aliased clones
    ///   (Rc-shared) all surface the same id, matching Python's `id()`
    ///   semantics for `b = a` aliasing (#305).
    /// - set: reads `obj_id` from the shared [`SetInner`] (same rationale).
    /// - str: uses the pool pointer address directly.
    ///
    /// Returns `None` for primitive types (callers handle those directly).
    pub fn value_id(&self) -> Option<i64> {
        // `as i64` wraps past 2^63; tracked separately, not specific to this
        // PR (tuple has the same shape).
        match top16(self.0) {
            TAG_TUPLE => Some(unsafe { self.tuple_inner() }.obj_id as i64),
            TAG_LIST => Some(unsafe { self.list_inner() }.obj_id as i64),
            TAG_STR => Some((self.0 & PAYLOAD_MASK) as i64),
            TAG_OPAQUE => match unsafe { &*self.opaque_ptr() } {
                // Small-tuple variants stash a monotonic obj_id alongside their
                // inline payload so `id()` stays stable across clones; see #281.
                Opaque::SmallTuple2 { obj_id, .. } => Some(*obj_id as i64),
                Opaque::SmallTuple3 { obj_id, .. } => Some(*obj_id as i64),
                // Sets are Rc-shared with an obj_id captured at construction;
                // aliased clones surface the same id (#305).
                Opaque::Set(rc) => Some(rc.obj_id as i64),
                // Dicts already share an Rc backing.  Surface the Rc pointer
                // address so `b = a; id(a) == id(b)` for dicts too (#305).
                Opaque::Dict(rc) => Some(Rc::as_ptr(rc) as i64),
                // BigInt clones share this opaque slot and the inner Rc.  Keep
                // using the Rc address for Python object identity
                // (`b = a; a is b`) parity (#523).
                Opaque::PyBigInt(rc) => Some(Rc::as_ptr(rc) as i64),
                // Generator clones share the same Rc<RefCell<...>>; surface its
                // pointer address so `id(g)` is non-zero and stable, and so
                // `g is iter(g)` can be backed by ptr equality (#714).
                Opaque::Generator(rc) => Some(Rc::as_ptr(rc) as i64),
                // Bytes and PyModule share Rc backing across clones; use the
                // Rc pointer address so `b = a; id(a) == id(b)` holds (#722).
                Opaque::Bytes(rc) => Some(Rc::as_ptr(rc) as i64),
                Opaque::PyModule(rc) => Some(Rc::as_ptr(rc) as i64),
                // Regular and builtin functions share the same
                // Rc<UserFunction> representation. Builtin functions may now
                // have multiple live objects with the same dispatch name, so
                // their Python id must use this concrete Rc identity.
                Opaque::UserFunction(rc) => Some(Rc::as_ptr(rc) as i64),
                // BoundMethod / ClassBoundMethod / SuperProxy / SuperProxyClass:
                // each allocation gets a unique monotonic obj_id so that
                // `a = obj.method; a is a` is True while
                // `obj.method is obj.method` is False (#722).
                Opaque::BoundMethod { obj_id, .. } => Some(*obj_id as i64),
                Opaque::ClassBoundMethod { obj_id, .. } => Some(*obj_id as i64),
                Opaque::SuperProxy { obj_id, .. } => Some(*obj_id as i64),
                Opaque::SuperProxyClass { obj_id, .. } => Some(*obj_id as i64),
                Opaque::SuperProxyUnbound { obj_id, .. } => Some(*obj_id as i64),
                // BuiltinObject: the Rc<RefCell<...>> state is shared across
                // clones; its address is a stable, per-object id (#722).
                Opaque::BuiltinObject { state, .. } => Some(Rc::as_ptr(state) as i64),
                _ => None,
            },
            _ => None,
        }
    }

    /// The single typed identity key consumed by both `is` and `id()`.
    ///
    /// Allocation addresses, monotonic ids, inline boxes, floats, complexes,
    /// and target-identity built-in proxies are distinct variants.  Equality
    /// can therefore never merge two representations merely because their
    /// raw `u64` payloads happen to match.
    fn object_identity(&self) -> ObjectIdentity {
        debug_assert!(
            !self.is_unset(),
            "Value::object_identity() called on an uninitialised register slot"
        );

        if self.is_float() {
            return ObjectIdentity::Float(self.0);
        }
        if self.is_not_implemented() || self.is_ellipsis() {
            return ObjectIdentity::RawValue(self.0);
        }

        match top16(self.0) {
            TAG_NONE | TAG_BOOL | TAG_INT => ObjectIdentity::RawValue(self.0),
            TAG_STR if str_is_inline_bits(self.0) => ObjectIdentity::RawValue(self.0),
            TAG_STR => ObjectIdentity::Allocation(self.0 & PAYLOAD_MASK),
            TAG_TUPLE => ObjectIdentity::Counter(unsafe { self.tuple_inner() }.obj_id),
            TAG_LIST => ObjectIdentity::Counter(unsafe { self.list_inner() }.obj_id),
            TAG_OPAQUE => match unsafe { &*self.opaque_ptr() } {
                Opaque::PyBigInt(rc) => ObjectIdentity::Allocation(Rc::as_ptr(rc) as u64),
                Opaque::Dict(rc) => ObjectIdentity::Allocation(Rc::as_ptr(rc) as u64),
                Opaque::Set(rc) => ObjectIdentity::Counter(rc.obj_id),
                // Range aliases share this refcounted opaque slot; separately
                // constructed equal ranges have distinct slots.
                Opaque::Range { .. } => {
                    ObjectIdentity::Allocation(unsafe { self.opaque_slot_ptr() } as u64)
                }
                Opaque::BigRange(rc) => ObjectIdentity::Allocation(Rc::as_ptr(rc) as u64),
                Opaque::UserFunction(rc) => {
                    ObjectIdentity::Allocation(Rc::as_ptr(rc) as u64)
                }
                Opaque::PyClass(rc) => ObjectIdentity::Allocation(Rc::as_ptr(rc) as u64),
                Opaque::PyInstance(rc) => ObjectIdentity::Allocation(Rc::as_ptr(rc) as u64),
                Opaque::PyModule(rc) => ObjectIdentity::Allocation(Rc::as_ptr(rc) as u64),
                Opaque::BoundMethod { obj_id, .. }
                | Opaque::ClassBoundMethod { obj_id, .. }
                | Opaque::SuperProxy { obj_id, .. }
                | Opaque::SuperProxyClass { obj_id, .. }
                | Opaque::SuperProxyUnbound { obj_id, .. } => ObjectIdentity::Counter(*obj_id),
                Opaque::Generator(rc) => ObjectIdentity::Allocation(Rc::as_ptr(rc) as u64),
                Opaque::Bytes(rc) => ObjectIdentity::Allocation(Rc::as_ptr(rc) as u64),
                Opaque::Complex(re, im) => ObjectIdentity::Complex {
                    real: re.to_bits(),
                    imag: im.to_bits(),
                },
                Opaque::SmallTuple2 { obj_id, .. } | Opaque::SmallTuple3 { obj_id, .. } => {
                    ObjectIdentity::Counter(*obj_id)
                }
                Opaque::BuiltinObject { ops, state } => {
                    match ops.identity_payload(state) {
                        Some(payload) => ObjectIdentity::Builtin {
                            type_id: (*ops).type_id(),
                            payload,
                        },
                        None => ObjectIdentity::Allocation(Rc::as_ptr(state) as u64),
                    }
                }
            },
            _ => unreachable!("invalid NaN-box tag in object identity"),
        }
    }

    /// Exact Python object-identity comparison.
    ///
    /// This is allocation-free.  The interpreter's `is` / `is not` operators
    /// delegate here so they cannot drift from [`Value::object_id`].
    #[inline]
    pub fn is_identical_to(&self, other: &Value) -> bool {
        self.object_identity() == other.object_identity()
    }

    /// Python object identity — the non-negative integer `id()` reports.
    ///
    /// [`Value::is_identical_to`] and this method consume the same typed key.
    /// Its injective numeric encoding gives the total contract
    ///
    /// ```text
    /// a.is_identical_to(b) == (a.object_id() == b.object_id())
    /// ```
    ///
    /// Allocation-backed values retain their existing `u64` address id.
    /// Counter, raw-box, float, complex, and custom built-in identities occupy
    /// disjoint arbitrary-precision namespaces; none relies on hashing.
    pub fn object_id(&self) -> Value {
        match self.object_identity().encode() {
            EncodedObjectIdentity::Unsigned(id) => Value::uint(id),
            EncodedObjectIdentity::Wide(id) => Value::bigint(id),
        }
    }

    // ── Private unsafe helpers ───────────────────────────────────────────────

    unsafe fn str_hdr(&self) -> *const u8 {
        (self.0 & PAYLOAD_MASK) as *const u8
    }

    #[inline(always)]
    unsafe fn str_unicode_state_slot(&self) -> *mut usize {
        let hdr = unsafe { self.str_hdr() };
        let rc_type = unsafe { *(hdr as *const u32) };
        if rc_type & STR_TYPE_B == 0 {
            unsafe { hdr.add(8) as *mut usize }
        } else {
            unsafe { hdr.add(STR_SLICE_CACHE_OFFSET) as *mut usize }
        }
    }

    #[inline]
    fn str_unicode_len(&self) -> usize {
        debug_assert!(self.is_str() && !str_is_inline_bits(self.0));
        unsafe {
            let slot = self.str_unicode_state_slot();
            let state = *slot;
            if state & STR_CODEPOINT_LEN_TAG != 0 {
                return state >> STR_CODEPOINT_LEN_SHIFT;
            }
            if state != 0 {
                return (*(state as *const StrUnicodeCache)).codepoint_len as usize;
            }

            let len = utf8_codepoint_count(self.str_as_str().as_bytes());
            slot.write((len << STR_CODEPOINT_LEN_SHIFT) | STR_CODEPOINT_LEN_TAG);
            len
        }
    }

    #[inline]
    fn str_unicode_len_for_index(&self) -> usize {
        debug_assert!(self.is_str() && !str_is_inline_bits(self.0));
        unsafe {
            let slot = self.str_unicode_state_slot();
            let state = *slot;
            if state != 0 && state & STR_CODEPOINT_LEN_TAG == 0 {
                return (*(state as *const StrUnicodeCache)).codepoint_len as usize;
            }
            if state & STR_CODEPOINT_LEN_TAG != 0 {
                let len = state >> STR_CODEPOINT_LEN_SHIFT;
                if state & STR_CODEPOINT_INDEX_SEEN == 0 {
                    slot.write(state | STR_CODEPOINT_INDEX_SEEN);
                    return len;
                }
                return self.str_unicode_cache().codepoint_len as usize;
            }

            let len = utf8_codepoint_count(self.str_as_str().as_bytes());
            slot.write(
                (len << STR_CODEPOINT_LEN_SHIFT) | STR_CODEPOINT_LEN_TAG | STR_CODEPOINT_INDEX_SEEN,
            );
            len
        }
    }

    #[inline]
    fn str_unicode_cache(&self) -> &StrUnicodeCache {
        debug_assert!(self.is_str() && !str_is_inline_bits(self.0));
        unsafe {
            let slot = self.str_unicode_state_slot();
            let state = *slot;
            if state != 0 && state & STR_CODEPOINT_LEN_TAG == 0 {
                return &*(state as *const StrUnicodeCache);
            }
            let cache = StrUnicodeCache::build(self.str_as_str());
            if state & STR_CODEPOINT_LEN_TAG != 0 {
                debug_assert_eq!(
                    cache.codepoint_len as usize,
                    state >> STR_CODEPOINT_LEN_SHIFT
                );
            }
            let cache = Box::into_raw(cache);
            slot.write(cache as usize);
            &*cache
        }
    }

    unsafe fn str_as_bytes(&self) -> &[u8] {
        // Inline (SSO, #2832): the bytes live in this value's own NaN-box
        // payload, starting one byte in (past the marker/length byte).  The
        // returned slice borrows from `self`, so its lifetime is bounded by
        // `&self` — sound because a moved value can't be borrowed concurrently.
        if str_is_inline_bits(self.0) {
            let len = ((self.0 >> 1) & 0b111) as usize;
            unsafe {
                let ptr = (&self.0 as *const u64 as *const u8).add(1);
                return std::slice::from_raw_parts(ptr, len);
            }
        }
        unsafe {
            let hdr = self.str_hdr();
            let sub_len = *(hdr.add(4) as *const u32) as usize;
            let rc_type = *(hdr as *const u32);
            let ref_ptr = if rc_type & STR_TYPE_B == 0 {
                hdr.add(16)
            } else {
                *(hdr.add(8) as *const *const u8)
            };
            std::slice::from_raw_parts(ref_ptr, sub_len)
        }
    }

    unsafe fn str_as_str(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(self.str_as_bytes()) }
    }

    /// Raw pointer to the shared [`TupleInner`] backing.  Caller must guarantee
    /// `self` is a TAG_TUPLE value (#2268).
    unsafe fn tuple_inner_ptr(&self) -> *const TupleInner {
        (self.0 & PAYLOAD_MASK) as *const TupleInner
    }

    /// Borrow the inner tuple header.  SAFETY: `self` must be a TAG_TUPLE value
    /// and the Rc must be live (which it is for any reachable `Value`).
    unsafe fn tuple_inner(&self) -> &TupleInner {
        unsafe { &*self.tuple_inner_ptr() }
    }

    /// Raw pointer to the shared [`ListInner`] backing.  Caller must guarantee
    /// `self` is a TAG_LIST value.
    unsafe fn list_inner_ptr(&self) -> *const ListInner {
        (self.0 & PAYLOAD_MASK) as *const ListInner
    }

    /// Borrow the inner list header.  SAFETY: `self` must be a TAG_LIST value
    /// and the Rc must be live (which it is for any reachable `Value`).
    unsafe fn list_inner(&self) -> &ListInner {
        unsafe { &*self.list_inner_ptr() }
    }

    unsafe fn opaque_slot_ptr(&self) -> *mut OpaqueSlot {
        (self.0 & PAYLOAD_MASK) as *mut _
    }

    unsafe fn opaque_ptr(&self) -> *mut Opaque {
        unsafe { std::ptr::addr_of_mut!((*self.opaque_slot_ptr()).value) }
    }

    // ── Public accessors ─────────────────────────────────────────────────────

    pub fn as_bool(&self) -> bool {
        debug_assert!(
            !self.is_unset(),
            "Value::as_bool() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        (self.0 & 1) != 0
    }

    pub fn as_int_raw(&self) -> i64 {
        debug_assert!(
            !self.is_unset(),
            "Value::as_int_raw() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        let raw = (self.0 & PAYLOAD_MASK) as i64;
        if self.0 & INT_SIGN_BIT != 0 {
            raw | !PAYLOAD_MASK as i64
        } else {
            raw
        }
    }

    pub fn as_float_raw(&self) -> f64 {
        debug_assert!(
            !self.is_unset(),
            "Value::as_float_raw() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        f64::from_bits(self.0)
    }

    pub fn as_str(&self) -> Option<&str> {
        debug_assert!(
            !self.is_unset(),
            "Value::as_str() called on an uninitialised register slot (Value::unset()). \
             A CheckLocal instruction is missing for this read."
        );
        if self.is_str() {
            Some(unsafe { self.str_as_str() })
        } else {
            None
        }
    }

    /// O(1) cached ASCII-ness for a string value.
    ///
    /// The flag is computed eagerly at construction (`Value::string`) for almost
    /// every string, and propagated for ASCII parents in `string_slice`.  When a
    /// header carries no computed flag yet (a slice of a non-ASCII string), it is
    /// computed on first call and cached in-place so subsequent queries are O(1).
    /// Strings are immutable, so the cached flag never goes stale.
    ///
    /// Returns `false` for non-string values (the debug_assert guards misuse).
    pub fn str_is_ascii(&self) -> bool {
        debug_assert!(
            self.is_str(),
            "Value::str_is_ascii() called on a non-string value"
        );
        if !self.is_str() {
            return false;
        }
        // Inline (SSO, #2832): no header flag; scan the ≤ 5 bytes directly.
        if str_is_inline_bits(self.0) {
            return unsafe { self.str_as_str() }.is_ascii();
        }
        let hdr = (self.0 & PAYLOAD_MASK) as *mut u32;
        let rc_type = unsafe { *hdr };
        if rc_type & STR_ASCII_COMPUTED != 0 {
            return rc_type & STR_IS_ASCII != 0;
        }
        // Uncomputed (only reachable for slices of a non-ASCII parent): scan once
        // and cache the result.  Single-threaded runtime, so the in-place header
        // write through the raw pointer is sound.
        let is_ascii = unsafe { self.str_as_str() }.is_ascii();
        let new_flag = if is_ascii {
            STR_IS_ASCII | STR_ASCII_COMPUTED
        } else {
            STR_ASCII_COMPUTED
        };
        unsafe { *hdr = rc_type | new_flag };
        is_ascii
    }

    /// Return the Python codepoint length of a string.
    ///
    /// ASCII strings use their byte length directly. Non-ASCII strings build a
    /// tagged codepoint length on first use; subsequent calls are O(1). It
    /// counts leading bytes rather than using `str::chars()` so pyrust's
    /// CESU-8-encoded lone surrogates remain one Python codepoint each.
    pub fn str_codepoint_len(&self) -> usize {
        debug_assert!(
            self.is_str(),
            "Value::str_codepoint_len() called on a non-string value"
        );
        if !self.is_str() {
            return 0;
        }
        let s = unsafe { self.str_as_str() };
        if str_is_inline_bits(self.0) {
            return utf8_codepoint_count(s.as_bytes());
        }
        if self.str_is_ascii() {
            return s.len();
        }
        self.str_unicode_len()
    }

    /// Return the codepoint length for an index or slice operation.
    ///
    /// Keeping this separate from [`Self::str_codepoint_len`] avoids allocating
    /// an offset table for workloads that only ask for length. The first indexed
    /// access records reuse and scans directly; a second access promotes the
    /// header to the sparse offset cache.
    pub fn str_codepoint_len_for_index(&self) -> usize {
        debug_assert!(
            self.is_str(),
            "Value::str_codepoint_len_for_index() called on a non-string value"
        );
        if !self.is_str() {
            return 0;
        }
        let s = unsafe { self.str_as_str() };
        if str_is_inline_bits(self.0) {
            return utf8_codepoint_count(s.as_bytes());
        }
        if self.str_is_ascii() {
            return s.len();
        }
        self.str_unicode_len_for_index()
    }

    /// Translate a Python codepoint boundary to its UTF-8/CESU-8 byte offset.
    ///
    /// `index` may equal [`Self::str_codepoint_len`], in which case this returns
    /// the byte length. A first non-ASCII lookup scans from the nearer end;
    /// reused strings start at the nearest sparse checkpoint and advance at most
    /// `STR_OFFSET_STRIDE - 1` codepoints.
    pub fn str_codepoint_byte_offset(&self, index: usize) -> usize {
        assert!(
            self.is_str(),
            "Value::str_codepoint_byte_offset() called on a non-string value"
        );
        let s = unsafe { self.str_as_str() };
        if str_is_inline_bits(self.0) {
            let len = utf8_codepoint_count(s.as_bytes());
            assert!(index <= len, "string codepoint offset out of bounds");
            return uncached_utf8_byte_offset(s.as_bytes(), index, len);
        }
        if self.str_is_ascii() {
            assert!(index <= s.len(), "string codepoint offset out of bounds");
            return index;
        }
        let state = unsafe { *self.str_unicode_state_slot() };
        if state & STR_CODEPOINT_LEN_TAG != 0 {
            let len = state >> STR_CODEPOINT_LEN_SHIFT;
            assert!(index <= len, "string codepoint offset out of bounds");
            return uncached_utf8_byte_offset(s.as_bytes(), index, len);
        }
        if state == 0 {
            self.str_unicode_len();
            return self.str_codepoint_byte_offset(index);
        }
        let cache = unsafe { &*(state as *const StrUnicodeCache) };
        assert!(
            index <= cache.codepoint_len as usize,
            "string codepoint offset out of bounds"
        );
        cache.byte_offset(s, index)
    }

    /// Return the byte range occupied by one Python codepoint.
    pub fn str_codepoint_byte_range(&self, index: usize) -> (usize, usize) {
        // `str_codepoint_byte_offset` performs the release-active tag and
        // index checks before either helper accesses raw string storage.
        let start = self.str_codepoint_byte_offset(index);
        let s = unsafe { self.str_as_str() };
        assert!(start < s.len(), "string codepoint index out of bounds");
        (start, start + utf8_codepoint_width(s.as_bytes()[start]))
    }

    /// Grow this string in place by appending `other`'s bytes, the pyrust
    /// equivalent of CPython's `_PyUnicode_Append` fast path (issue #2850).
    ///
    /// Returns `true` when the append happened in place (the value's backing was
    /// `realloc`'d and this `Value` now denotes the concatenation); returns
    /// `false` — leaving `self` untouched — whenever the string does not qualify,
    /// in which case the caller must fall back to a fresh `Value::string` concat:
    ///
    ///   * inline (SSO) strings — no heap backing, no refcount to check;
    ///   * Layout B slice descriptors — the backing is shared with the parent;
    ///   * strings with refcount > 1 — another live `Value` (an alias, or the
    ///     intern table) shares the backing, so mutating in place would corrupt
    ///     it (this is the whole aliasing-safety mechanism, and it also covers
    ///     interned strings, which the table holds a second reference to).
    ///
    /// This is what makes `s += t` in a loop O(n) instead of O(n²): the backing
    /// is `realloc`'d (typically in place, amortised O(1)) rather than a brand
    /// new buffer being allocated and the whole left operand copied every time.
    ///
    /// A non-string receiver is rejected before raw storage is accessed. The
    /// thread-bound `Value` type prevents concurrent access to the header, and
    /// the refcount==1 check guarantees this is the only value referencing the
    /// backing.
    pub fn str_append_in_place(&mut self, other: &str) -> bool {
        if !self.is_str() {
            return false;
        }
        // Inline (SSO): bytes live in the NaN-box, no heap backing to grow.
        if str_is_inline_bits(self.0) {
            return false;
        }
        let hdr = (self.0 & PAYLOAD_MASK) as *mut u8;
        let rc_type = unsafe { *(hdr as *const u32) };
        // Layout B (slice descriptor) shares the parent's byte buffer — never
        // mutate it in place.
        if rc_type & STR_TYPE_B != 0 {
            return false;
        }
        // Refcount must be exactly 1 (bits 31:3). Any alias or the intern table
        // holding a second reference bumps this above 1 → bail to fresh concat.
        if rc_type >> 3 != 1 {
            return false;
        }
        // Appending nothing keeps the value bit-identical; treat as a no-op
        // success so the caller stores `self` back unchanged.
        if other.is_empty() {
            return true;
        }
        let old_len = unsafe { *(hdr.add(4) as *const u32) } as usize;
        let add_len = other.len();
        let Some(new_len) = old_len.checked_add(add_len) else {
            return false;
        };
        // Guard against the u32 sub_len field overflowing (pathological only).
        if new_len > u32::MAX as usize {
            return false;
        }
        let old_layout = nanbox_owned_string_layout(old_len);
        let Some(new_size) = STR_OWNED_HEADER_SIZE.checked_add(new_len) else {
            return false;
        };
        let new_ptr = unsafe { realloc(hdr, old_layout, new_size) };
        if new_ptr.is_null() {
            // realloc failure leaves the original block intact; fall back.
            return false;
        }
        // Validate before doing any further work.  This is release-active and
        // aborts on an unrepresentable address; unwinding here would be
        // unsound because a successful realloc has already invalidated `hdr`.
        let new_bits = encode_nanbox_heap_pointer(TAG_STR_BITS, new_ptr);
        unsafe {
            new_ptr
                .add(STR_OWNED_HEADER_SIZE + old_len)
                .copy_from_nonoverlapping(other.as_bytes().as_ptr(), add_len);
            (new_ptr.add(4) as *mut u32).write(new_len as u32);
            // Update the cached ASCII flag: the result is ASCII iff the old
            // string's flag was ASCII (or unknown/uncomputed and rescanned) and
            // the appended bytes are ASCII. Simplest correct rule: recompute the
            // combined flag from the old flag and `other`.
            let old_rc_type = *(new_ptr as *const u32);
            let old_ascii =
                (old_rc_type & STR_ASCII_COMPUTED != 0) && (old_rc_type & STR_IS_ASCII != 0);
            let old_computed = old_rc_type & STR_ASCII_COMPUTED != 0;
            // Preserve rc/type bits, clear the ascii bits, then set fresh ones.
            let base = old_rc_type & !(STR_IS_ASCII | STR_ASCII_COMPUTED);
            let new_flag = if old_computed {
                // Old ASCII-ness was known: combine with `other`'s ASCII-ness.
                if old_ascii && other.is_ascii() {
                    STR_IS_ASCII | STR_ASCII_COMPUTED
                } else {
                    STR_ASCII_COMPUTED
                }
            } else {
                // Old flag was never computed (a slice of a non-ASCII parent
                // that was later grown — rare). Leave it uncomputed for a lazy
                // resolve on first query.
                0
            };
            *(new_ptr as *mut u32) = base | new_flag;

            // The cached length/checkpoints describe the pre-append bytes.
            // Drop them only after realloc succeeds, then rebuild lazily if a
            // later Unicode operation needs them.
            let state_slot = new_ptr.add(8) as *mut usize;
            let state = *state_slot;
            if state != 0 && state & STR_CODEPOINT_LEN_TAG == 0 {
                drop(Box::from_raw(state as *mut StrUnicodeCache));
            }
            state_slot.write(0);
        }
        self.0 = new_bits;
        true
    }
}
