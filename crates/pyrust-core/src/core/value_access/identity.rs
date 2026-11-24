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

    /// Tag-only `BuiltinObject` classification: `true` for the extension types
    /// carried by a `BuiltinTypeOps` table (`frozenset`, `bytearray`,
    /// `mappingproxy`, dict views, …).  Unlike `kind()`, this never builds a
    /// `ValueKind` or borrows the object's state, so operator dispatch can
    /// cheaply skip built-in-object special cases for every other value.
    #[inline(always)]
    pub fn is_builtin_object(&self) -> bool {
        top16(self.0) == TAG_OPAQUE
            && matches!(unsafe { &*self.opaque_ptr() }, Opaque::BuiltinObject { .. })
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

    /// Fast equality for two scalar NaN-box values with identical bits.
    /// This includes the identity case for a boxed NaN and the common exact
    /// int/bool/str/float pair, while conservatively rejecting every opaque or
    /// container representation.
    #[inline]
    pub fn scalar_bits_equal(&self, other: &Self) -> bool {
        self.0 == other.0 && self.cannot_user_eq()
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
            TAG_STR => ObjectIdentity::allocation_from_nanbox(self.0 & PAYLOAD_MASK),
            TAG_TUPLE => ObjectIdentity::Counter(unsafe { self.tuple_inner() }.obj_id),
            TAG_LIST => ObjectIdentity::Counter(unsafe { self.list_inner() }.obj_id),
            TAG_OPAQUE => match unsafe { &*self.opaque_ptr() } {
                Opaque::PyBigInt(rc) => ObjectIdentity::allocation_from_ptr(Rc::as_ptr(rc)),
                Opaque::Dict(rc) => ObjectIdentity::allocation_from_ptr(Rc::as_ptr(rc)),
                Opaque::Set(rc) => ObjectIdentity::Counter(rc.obj_id),
                // Range aliases share this refcounted opaque slot; separately
                // constructed equal ranges have distinct slots.
                Opaque::Range { .. } => {
                    ObjectIdentity::allocation_from_ptr(unsafe { self.opaque_slot_ptr() })
                }
                Opaque::BigRange(rc) => ObjectIdentity::allocation_from_ptr(Rc::as_ptr(rc)),
                Opaque::UserFunction(rc) => ObjectIdentity::allocation_from_ptr(Rc::as_ptr(rc)),
                Opaque::PyClass(rc) => ObjectIdentity::allocation_from_ptr(Rc::as_ptr(rc)),
                Opaque::PyInstance(rc) => ObjectIdentity::allocation_from_ptr(Rc::as_ptr(rc)),
                Opaque::PyModule(rc) => ObjectIdentity::allocation_from_ptr(Rc::as_ptr(rc)),
                Opaque::BoundMethod { obj_id, .. }
                | Opaque::ClassBoundMethod { obj_id, .. }
                | Opaque::SuperProxy { obj_id, .. }
                | Opaque::SuperProxyClass { obj_id, .. }
                | Opaque::SuperProxyUnbound { obj_id, .. } => ObjectIdentity::Counter(*obj_id),
                Opaque::Generator(rc) => ObjectIdentity::allocation_from_ptr(Rc::as_ptr(rc)),
                Opaque::Bytes(rc) => ObjectIdentity::allocation_from_ptr(Rc::as_ptr(rc)),
                Opaque::Complex(re, im) => ObjectIdentity::Complex {
                    real: re.to_bits(),
                    imag: im.to_bits(),
                },
                Opaque::SmallTuple2 { obj_id, .. } | Opaque::SmallTuple3 { obj_id, .. } => {
                    ObjectIdentity::Counter(*obj_id)
                }
                Opaque::BuiltinObject { ops, state } => match ops.identity_payload(state) {
                    Some(payload) => ObjectIdentity::Builtin {
                        type_id: (*ops).type_id(),
                        payload,
                    },
                    None => ObjectIdentity::allocation_from_ptr(Rc::as_ptr(state)),
                },
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
    /// Common allocation-backed values retain their existing address id.
    /// Other typed identities receive stable, bounded ids from a collision-safe
    /// side table; no identity payload is folded or truncated.
    pub fn object_id(&self) -> Value {
        Value::int(self.object_identity().encode() as i64)
    }
}
