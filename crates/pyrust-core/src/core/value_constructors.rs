impl Value {
    // ── Constructors ─────────────────────────────────────────────────────────

    pub fn none() -> Self {
        Value::from_bits(TAG_NONE_BITS)
    }

    /// A distinct, internal-only sentinel representing "register slot not
    /// initialised". The bit pattern is a specific positive NaN that the VM
    /// never produces from real Python values — `0x7FF8_0000_0000_BAD0`.
    ///
    /// `is_unset()` returns true only for this exact pattern.
    ///
    /// Reading an unset slot through `kind()`, `truthy_raw()`, or any accessor
    /// that routes through `kind()` will panic in debug builds (via
    /// `debug_assert!`).  In release builds the assert is elided; the runtime
    /// tripwire is the compiler's `Insn::CheckLocal` emission.  Do not pass an
    /// unset `Value` to any accessor other than `is_unset()` / `as_some()`.
    pub fn unset() -> Self {
        Value::from_bits(UNSET_BITS)
    }

    pub fn is_unset(&self) -> bool {
        self.0 == UNSET_BITS
    }

    /// `Some(self)` if this slot has been written, else `None`.
    /// Useful for migrating call sites that previously held `Option<Value>`.
    #[inline]
    pub fn as_some(&self) -> Option<&Value> {
        if self.is_unset() { None } else { Some(self) }
    }

    #[inline]
    pub fn as_some_mut(&mut self) -> Option<&mut Value> {
        if self.is_unset() { None } else { Some(self) }
    }

    pub fn bool_(b: bool) -> Self {
        Value::from_bits(TAG_BOOL_BITS | b as u64)
    }

    pub fn int(n: i64) -> Self {
        const MAX_I48: i64 = (1 << 47) - 1;
        const MIN_I48: i64 = -(1 << 47);
        if (MIN_I48..=MAX_I48).contains(&n) {
            Value::from_bits(TAG_INT_BITS | (n as u64 & PAYLOAD_MASK))
        } else {
            Value::opaque(Opaque::PyBigInt(Rc::new(BigInt::from(n))))
        }
    }

    /// Box an *unsigned* 64-bit integer as a Python `int`.
    ///
    /// [`Value::int`] takes an `i64`, so anything above `i64::MAX` would wrap
    /// negative.  Object ids are unsigned — a non-float immediate carries its
    /// tag in the top bits (#2956) — and CPython's `id()` is likewise always
    /// non-negative, so promote those to a big int instead.
    pub fn uint(n: u64) -> Self {
        match i64::try_from(n) {
            Ok(n) => Value::int(n),
            Err(_) => Value::bigint(BigInt::from(n)),
        }
    }

    pub fn bigint(n: BigInt) -> Self {
        Value::opaque(Opaque::PyBigInt(Rc::new(n)))
    }

    /// Box an `f64`.
    ///
    /// Every NaN gets a *fresh* object identity (see [`mint_nan_identity`]) so
    /// that distinct NaNs behave like distinct CPython objects in dicts, sets
    /// and sequence comparisons (#2911).  Non-NaN floats are stored verbatim,
    /// so this is bit-for-bit unchanged on the arithmetic hot path — the
    /// `is_nan()` branch was already here.
    ///
    /// Use [`Value::float_from_bits`] instead when *restoring* a float whose
    /// identity must be preserved (e.g. rebuilding a `PyKey::Float`).
    pub fn float(f: f64) -> Self {
        if f.is_nan() {
            Value::from_bits(mint_nan_identity())
        } else {
            Value::from_bits(f.to_bits())
        }
    }

    /// Rebuild a float from the exact bit pattern held by a `PyKey::Float`,
    /// preserving NaN object identity.
    ///
    /// [`Value::float`] mints a fresh identity for every NaN, so reconstructing
    /// a container key with it would hand back a NaN that is no longer the
    /// object the container stores — `list(d)[0] is n` would be `False` and
    /// `d[list(d)[0]]` would raise `KeyError`.  Dict/set iteration, `keys()`,
    /// and the frozen-key-order walk must all round-trip through here.
    ///
    /// Patterns that are not a minted NaN are normalised through
    /// [`Value::float`], which keeps a stale or hostile pattern from aliasing a
    /// pointer tag; for every ordinary float that normalisation is the identity
    /// function.
    pub fn float_from_bits(bits: u64) -> Self {
        if is_minted_nan(bits) {
            return Value::from_bits(bits);
        }
        Value::float(f64::from_bits(bits))
    }

    pub fn string(s: impl AsRef<str>) -> Self {
        let s = s.as_ref();
        let len = s.len();
        // Small-string optimisation (#2832): store ≤ 5 bytes inline in the
        // NaN-box payload, skipping the heap allocation entirely.
        if len <= STR_INLINE_MAX {
            return make_inline_str(s);
        }
        // Layout A: [rc_type:u32][sub_len:u32][unicode_state:usize][bytes]
        //            offset 0     offset 4     offset 8            offset 16
        //
        // Owned bytes always start at header+16, so the old self-referential
        // `ref` field was redundant.  Reuse those eight bytes for a lazily
        // allocated non-ASCII length/offset cache.
        let layout = nanbox_owned_string_layout(len);
        let ptr = unsafe { alloc_or_handle(layout) };
        // Compute ASCII-ness once, here, where every byte is about to be touched
        // by the memcpy anyway.  This covers ~all string construction (concat,
        // join, format, decode, chr, repeat, upper, replace, … all funnel
        // through `Value::string`), so the cached flag is set eagerly.
        let ascii_flag = if s.is_ascii() {
            STR_IS_ASCII | STR_ASCII_COMPUTED
        } else {
            STR_ASCII_COMPUTED
        };
        unsafe {
            (ptr as *mut u32).write(STR_RC_ONE | ascii_flag); // rc=1, type=A
            (ptr.add(4) as *mut u32).write(len as u32);
            (ptr.add(8) as *mut usize).write(0);
            if len > 0 {
                ptr.add(STR_OWNED_HEADER_SIZE)
                    .copy_from_nonoverlapping(s.as_bytes().as_ptr(), len);
            }
        }
        Value::from_bits(encode_nanbox_heap_pointer(TAG_STR_BITS, ptr))
    }

    /// Build a string of exactly `total` bytes by filling its backing buffer
    /// in place, avoiding the intermediate `String` allocation + memcpy that
    /// `Value::string` would otherwise pay.  `fill` receives the uninitialised
    /// byte buffer and must write exactly `total` bytes; it returns
    /// `Some(is_ascii)` to set the cached ASCII flag from that same (cache-hot)
    /// pass, or `None` to leave it uncomputed for a lazy resolve — used by long
    /// joins where a separate ASCII scan would double the bytes touched relative
    /// to the copy alone.
    /// Used by `str.join` so the result bytes are touched once (the fill copy)
    /// rather than three times (push into a `String`, `is_ascii` scan, then the
    /// final memcpy in `Value::string`).
    ///
    /// # Safety
    /// `fill` must initialise all `total` bytes, the resulting buffer must be
    /// valid UTF-8, and any `Some(_)` it returns must report the true
    /// ASCII-ness.  Callers join already-validated `&str` parts, so these hold.
    pub unsafe fn string_from_fill(
        total: usize,
        fill: impl FnOnce(&mut [u8]) -> Option<bool>,
    ) -> Self {
        // Small-string optimisation (#2832): a ≤ 5-byte result goes inline.
        // Route through the same path as every other constructor so identical
        // content always has identical bits (interning / `is` consistency).
        if total <= STR_INLINE_MAX {
            let mut buf = [0u8; STR_INLINE_MAX];
            fill(&mut buf[..total]);
            // SAFETY: `fill`'s contract is that it writes valid UTF-8 into the
            // whole buffer (callers join already-validated `&str` parts).
            let s = unsafe { std::str::from_utf8_unchecked(&buf[..total]) };
            return make_inline_str(s);
        }
        let layout = nanbox_owned_string_layout(total);
        unsafe {
            let ptr = alloc_or_handle(layout);
            (ptr.add(8) as *mut usize).write(0);
            let buf = std::slice::from_raw_parts_mut(ptr.add(STR_OWNED_HEADER_SIZE), total);
            let is_ascii = fill(buf);
            let ascii_flag = match is_ascii {
                Some(true) => STR_IS_ASCII | STR_ASCII_COMPUTED,
                Some(false) => STR_ASCII_COMPUTED,
                // Caller declined to compute it (e.g. a long join, where the
                // scan would double the bytes touched): leave it uncomputed and
                // let `str_is_ascii` resolve it lazily on first query.
                None => 0,
            };
            (ptr as *mut u32).write(STR_RC_ONE | ascii_flag); // rc=1, type=A
            (ptr.add(4) as *mut u32).write(total as u32);
            Value::from_bits(encode_nanbox_heap_pointer(TAG_STR_BITS, ptr))
        }
    }

    /// `str(n)` / `repr(n)` for an `i64` without the intermediate heap `String`
    /// that `n.to_string()` allocates (#alloc): format the digits into a stack
    /// buffer (an `i64` is at most 20 ASCII bytes including the sign) and copy
    /// them once into the string `Value`.  Used by `str()`, `repr()`, and
    /// f-string formatting for the common bare-integer case.
    pub fn int_string(n: i64) -> Self {
        use std::fmt::Write as _;
        // i64::MIN is "-9223372036854775808" — exactly 20 bytes.
        struct Buf {
            b: [u8; 20],
            n: usize,
        }
        impl std::fmt::Write for Buf {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                let bytes = s.as_bytes();
                self.b[self.n..self.n + bytes.len()].copy_from_slice(bytes);
                self.n += bytes.len();
                Ok(())
            }
        }
        let mut buf = Buf { b: [0; 20], n: 0 };
        let _ = write!(buf, "{n}");
        // SAFETY: the `Display` impl for `i64` emits only ASCII digits and an
        // optional leading '-', so the written prefix is valid UTF-8.
        Value::string(unsafe { std::str::from_utf8_unchecked(&buf.b[..buf.n]) })
    }

    pub fn string_slice(&self, byte_start: usize, byte_end: usize) -> Self {
        assert!(
            self.is_str(),
            "Value::string_slice() called on a non-string value"
        );
        // Guard against inverted indices: wrapping subtraction would produce a
        // colossal sub_len and the resulting slice descriptor would be invalid.
        assert!(
            byte_start <= byte_end,
            "string_slice: byte_start ({byte_start}) > byte_end ({byte_end})"
        );
        let bytes = unsafe { self.str_as_bytes() };
        assert!(
            byte_end <= bytes.len(),
            "string_slice: byte_end ({byte_end}) exceeds string byte length ({})",
            bytes.len()
        );
        assert!(
            utf8_codepoint_boundary(bytes, byte_start) && utf8_codepoint_boundary(bytes, byte_end),
            "string_slice: range {byte_start}..{byte_end} splits a codepoint"
        );
        let sub_len = byte_end - byte_start;
        // Small-string optimisation (#2832): a ≤ 5-byte result (every single
        // character, so this is the hot `s[i]` / char-iteration path) goes
        // inline — no Layout B pool allocation.  This is also *required* when
        // `self` is itself inline: its bytes live in the (movable) NaN-box, so
        // there is no stable address for a Layout B `ref` to point at.  An
        // inline source is always ≤ 5 bytes, so `sub_len <= STR_INLINE_MAX`
        // already covers it.
        if sub_len <= STR_INLINE_MAX {
            let s = unsafe { self.str_as_str() };
            return make_inline_str(&s[byte_start..byte_end]);
        }
        let hdr = (self.0 & PAYLOAD_MASK) as *const u8;
        let rc_type = unsafe { *(hdr as *const u32) };
        // Layout A owns bytes at hdr+16. Layout B's ref (offset 8) points to
        // this slice's bytes[0]. Add byte_start for the new slice.
        let self_ref = if rc_type & STR_TYPE_B == 0 {
            unsafe { hdr.add(STR_OWNED_HEADER_SIZE) }
        } else {
            unsafe { *(hdr.add(8) as *const *const u8) }
        };
        let new_ref = unsafe { self_ref.add(byte_start) };

        // Find A_ptr (Layout A root) to increment its rc, and compute new offset.
        // Layout A: A_ptr = hdr,   new_offset = byte_start
        // Layout B: A_ptr = ref - stored_offset - 16,  new_offset = stored_offset + byte_start
        //
        // For Layout B→B chains the stored_offset already encodes the distance from A's
        // bytes[0] to this slice's bytes[0], so subtracting it (plus the 16-byte header)
        // from self_ref always recovers A_ptr without underflow.
        let (a_ptr, new_offset): (*mut u8, usize) = if rc_type & STR_TYPE_B == 0 {
            (hdr as *mut u8, byte_start)
        } else {
            let base = unsafe { *(hdr.add(16) as *const u32) as usize };
            // SAFETY: `base` is the byte distance from Layout A's bytes[0] to this
            // slice's bytes[0], written by a prior `string_slice` call.  Therefore
            // `self_ref == a_ptr + 16 + base` by construction, and the subtraction
            // `self_ref - (base + 16)` cannot underflow.  The `byte_start <= byte_end`
            // assert at entry guarantees we never produce an invalid descriptor, so
            // this invariant is preserved through any chain of slices.
            let root_distance = base
                .checked_add(STR_OWNED_HEADER_SIZE)
                .unwrap_or_else(|| abort_unrepresentable_nanbox_string_length(base));
            let a_ptr = unsafe { (self_ref as *mut u8).sub(root_distance) };
            debug_assert!(
                a_ptr as usize + STR_OWNED_HEADER_SIZE + base == self_ref as usize,
                "string_slice: Layout B offset mismatch — possible heap corruption"
            );
            let new_offset = base
                .checked_add(byte_start)
                .filter(|offset| *offset <= STR_MAX_BYTE_LEN)
                .unwrap_or_else(|| {
                    abort_unrepresentable_nanbox_string_length(base.saturating_add(byte_start))
                });
            (a_ptr, new_offset)
        };

        // Increment A.rc. A saturated 29-bit count becomes an immortal backing
        // allocation instead of wrapping or corrupting the packed flag bits.
        unsafe {
            let hdr_a = a_ptr as *mut u32;
            str_refcount_increment(hdr_a);
        }

        // Propagate ASCII-ness: a substring of an all-ASCII string is itself
        // all-ASCII, so we can mark the slice ASCII for free.  If the parent is
        // non-ASCII (or not yet computed), the slice *may* still be ASCII, so we
        // leave it uncomputed and let `str_is_ascii` resolve it lazily — never
        // mark a slice non-ASCII here, that would be a correctness bug.
        let ascii_flag = if rc_type & (STR_ASCII_COMPUTED | STR_IS_ASCII)
            == (STR_ASCII_COMPUTED | STR_IS_ASCII)
        {
            STR_IS_ASCII | STR_ASCII_COMPUTED
        } else {
            0
        };

        // Layout B:
        // [rc_type:u32][sub_len:u32][ref:*mut u8][offset:u32][pad:u32][state:usize]
        //  offset 0     offset 4     offset 8     offset 16   offset 20 offset 24
        // ref points directly to this slice's bytes[0]; ref - offset - 16 = A_ptr
        debug_assert!(sub_len <= STR_MAX_BYTE_LEN);
        debug_assert!(new_offset <= STR_MAX_BYTE_LEN);
        let ptr = unsafe { pool_b_alloc() };
        unsafe {
            (ptr as *mut u32).write(STR_RC_ONE | STR_TYPE_B | ascii_flag); // rc=1, type=B
            (ptr.add(4) as *mut u32).write(sub_len as u32);
            *(ptr.add(8) as *mut *const u8) = new_ref;
            (ptr.add(16) as *mut u32).write(new_offset as u32);
            (ptr.add(STR_SLICE_CACHE_OFFSET) as *mut usize).write(0);
        }
        Value::from_bits(encode_nanbox_heap_pointer(TAG_STR_BITS, ptr))
    }

    /// Construct a heap-tuple Value from an existing `Rc<TupleInner>` — used when
    /// multiple Values must share the same immutable backing (cloning).  Consumes
    /// one strong-count reference from `rc`; the matching drop happens in
    /// `Drop for Value` when `TAG_TUPLE` is observed (#2268).
    unsafe fn tuple_from_rc(rc: Rc<TupleInner>) -> Self {
        let raw = Rc::into_raw(rc);
        Value::from_bits(encode_nanbox_heap_pointer(TAG_TUPLE_BITS, raw))
    }

    /// Construct a new `list` Value.  Storage is an `Rc<ListInner>` so that
    /// `Value::clone` shares the backing — matching Python's reference
    /// semantics for mutable containers (#305).
    pub fn list(v: Vec<Value>) -> Self {
        let inner = Rc::new(ListInner {
            items: RefCell::new(v),
            obj_id: next_obj_id(),
        });
        unsafe { Self::list_from_rc(inner) }
    }

    /// Construct a list Value from an existing `Rc<ListInner>` — used when
    /// multiple Values must share the same backing list (e.g. cloning).
    /// Caller is responsible for incrementing the strong count *before*
    /// calling this if they want a logical alias rather than a move.
    ///
    /// SAFETY: consumes one strong-count reference from `rc`.  The matching
    /// drop happens in `Drop for Value` when `TAG_LIST` is observed.
    unsafe fn list_from_rc(rc: Rc<ListInner>) -> Self {
        let raw = Rc::into_raw(rc);
        Value::from_bits(encode_nanbox_heap_pointer(TAG_LIST_BITS, raw))
    }

    pub fn tuple(mut v: Vec<Value>) -> Self {
        // Small-tuple fast path (#281): route 2- and 3-element tuples through
        // `Opaque::SmallTuple2/3` so the backing `Vec<Value>` heap allocation
        // is avoided.  These shapes dominate hot sites (`dict.items()`,
        // `enumerate()`, `divmod()`, `str.partition()`, …).
        match v.len() {
            0 => {
                // Empty tuple: share one immutable singleton, so `() is ()` is
                // True (CPython parity, where `()` is a singleton) and an empty
                // `*args` collection costs no allocation.
                thread_local! {
                    static EMPTY_TUPLE: Value = {
                        let inner = Rc::new(TupleInner {
                            items: Vec::new(),
                            obj_id: next_obj_id(),
                        });
                        unsafe { Value::tuple_from_rc(inner) }
                    };
                }
                EMPTY_TUPLE.with(|t| t.clone())
            }
            2 => {
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                Value::opaque(Opaque::SmallTuple2 {
                    items: [a, b],
                    obj_id: next_obj_id(),
                })
            }
            3 => {
                let c = v.pop().unwrap();
                let b = v.pop().unwrap();
                let a = v.pop().unwrap();
                Value::opaque(Opaque::SmallTuple3 {
                    items: [a, b, c],
                    obj_id: next_obj_id(),
                })
            }
            _ => {
                let inner = Rc::new(TupleInner {
                    items: v,
                    obj_id: next_obj_id(),
                });
                unsafe { Self::tuple_from_rc(inner) }
            }
        }
    }

    pub fn dict(d: PyDict) -> Self {
        Value::opaque(Opaque::Dict(Rc::new(RefCell::new(d))))
    }

    pub fn set(s: PySet) -> Self {
        Value::opaque(Opaque::Set(Rc::new(SetInner {
            items: RefCell::new(s),
            obj_id: next_obj_id(),
        })))
    }

    pub fn bytes(b: Vec<u8>) -> Self {
        Value::opaque(Opaque::Bytes(Rc::new(b)))
    }

    pub fn complex(re: f64, im: f64) -> Self {
        Value::opaque(Opaque::Complex(re, im))
    }

    /// Construct a generic built-in object dispatched through the installed
    /// [`BuiltinTypeOps`] table.  `ops` must outlive the program (typically
    /// `&'static`); `state` is owned heap state of any concrete type.
    pub fn builtin_object(ops: &'static dyn BuiltinTypeOps, state: Box<dyn Any>) -> Self {
        Value::opaque(Opaque::BuiltinObject {
            ops,
            state: Rc::new(RefCell::new(state)),
        })
    }

    /// Construct a generic built-in object that shares state with an existing
    /// `BuiltinState` cell.  Used when multiple Values must reference the
    /// same underlying mutable state.
    pub fn builtin_object_shared(ops: &'static dyn BuiltinTypeOps, state: BuiltinState) -> Self {
        Value::opaque(Opaque::BuiltinObject { ops, state })
    }

    pub fn range(start: i64, stop: i64, step: i64) -> Self {
        Value::opaque(Opaque::Range { start, stop, step })
    }

    /// Construct a `range` from arbitrary-precision bounds (#2118).  When all
    /// three bounds fit in `i64` this collapses to the cheap i64-backed
    /// [`Self::range`]; otherwise it produces a [`Opaque::BigRange`].  This keeps
    /// the common small-range path unchanged (no `BigRange` allocation, no extra
    /// match arms exercised) and only pays for `BigInt` storage when needed.
    pub fn range_big(start: BigInt, stop: BigInt, step: BigInt) -> Self {
        match (start.to_i64(), stop.to_i64(), step.to_i64()) {
            (Some(s), Some(e), Some(st)) => Value::range(s, e, st),
            _ => Value::opaque(Opaque::BigRange(Rc::new(BigRangeData {
                start,
                stop,
                step,
            }))),
        }
    }

    pub fn user_function(f: Rc<UserFunction>) -> Self {
        Value::opaque(Opaque::UserFunction(f))
    }

    /// Construct a built-in function value.  Stored as a `UserFunction` with
    /// `kind = Builtin(name)` so the function machinery is unified (one Opaque
    /// variant for both user and built-in functions).  The per-name
    /// `UserFunction` stub is interned in a thread-local cache so repeated
    /// calls don't reallocate it — equivalent in cost to the previous
    /// single-pointer payload.
    fn interned_builtin_function_rc(name: &'static str) -> Rc<UserFunction> {
        thread_local! {
            static CACHE: RefCell<HashMap<&'static str, Rc<UserFunction>>>
                = RefCell::new(HashMap::new());
        }
        CACHE.with(|c| {
            if let Some(f) = c.borrow().get(name) {
                return Rc::clone(f);
            }
            let f = Rc::new(UserFunction {
                id: next_fn_id(),
                kind: UserFunctionKind::Builtin(name),
                name: Rc::from(name),
                qualname: Rc::from(name),
                name_overrides: RefCell::new(None),
                // Unset means the interpreter-owned registry supplies the
                // declaring module. An explicit deletion stores None.
                module: RefCell::new(Value::unset()),
                doc: RefCell::new(Value::none()),
                attrs: RefCell::new(None),
                annotations: RefCell::new(Value::unset()),
                defaults_override: RefCell::new(None),
                params: Vec::new(),
                param_binds: Rc::new(Vec::new()),
                memo_positional_parameter_count: 0,
                self_bind: None,
                local_names: Rc::new(HashSet::new()),
                local_index: Rc::new(HashMap::new()),
                global_names: Rc::new(HashSet::new()),
                nonlocal_names: Rc::new(HashSet::new()),
                env: Environment::new(None),
                is_memo_pure: false,
                precompiled_code: None,
                wrapped_func: None,
            });
            c.borrow_mut().insert(name, Rc::clone(&f));
            f
        })
    }

    pub fn builtin_function(name: &'static str) -> Self {
        Value::opaque(Opaque::UserFunction(Self::interned_builtin_function_rc(
            name,
        )))
    }

    /// Construct a distinct builtin callable object with the same immutable
    /// name-based dispatcher as the interned template. Immutable empty
    /// tables/environment are Rc-shared with the template; mutable state
    /// starts fresh.
    pub fn fresh_builtin_function(name: &'static str) -> Self {
        let template = Self::interned_builtin_function_rc(name);
        let function = Rc::new(UserFunction {
            id: next_fn_id(),
            kind: template.kind,
            name: Rc::clone(&template.name),
            qualname: Rc::clone(&template.qualname),
            name_overrides: RefCell::new(None),
            module: RefCell::new(Value::unset()),
            doc: RefCell::new(Value::none()),
            attrs: RefCell::new(None),
            annotations: RefCell::new(Value::unset()),
            defaults_override: RefCell::new(None),
            params: template.params.clone(),
            param_binds: Rc::clone(&template.param_binds),
            memo_positional_parameter_count: template.memo_positional_parameter_count,
            self_bind: template.self_bind,
            local_names: Rc::clone(&template.local_names),
            local_index: Rc::clone(&template.local_index),
            global_names: Rc::clone(&template.global_names),
            nonlocal_names: Rc::clone(&template.nonlocal_names),
            env: Rc::clone(&template.env),
            is_memo_pure: false,
            precompiled_code: None,
            wrapped_func: None,
        });
        Value::opaque(Opaque::UserFunction(function))
    }

    /// The `NotImplemented` singleton.  Stored as a reserved NaN-box bit
    /// pattern so identity comparison is a single u64 equality check.
    pub fn not_implemented() -> Self {
        Value::from_bits(NOT_IMPLEMENTED_BITS)
    }

    pub fn is_not_implemented(&self) -> bool {
        self.0 == NOT_IMPLEMENTED_BITS
    }

    pub fn ellipsis() -> Self {
        Value::from_bits(ELLIPSIS_BITS)
    }

    pub fn is_ellipsis(&self) -> bool {
        self.0 == ELLIPSIS_BITS
    }

    pub fn py_class(c: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::PyClass(c))
    }

    pub fn py_instance(i: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::PyInstance(i))
    }

    pub fn py_module(m: Rc<RefCell<PyModule>>) -> Self {
        Value::opaque(Opaque::PyModule(m))
    }

    pub fn bound_method(function: Rc<UserFunction>, receiver: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::BoundMethod {
            function,
            receiver,
            obj_id: next_obj_id(),
        })
    }

    /// Wrap a function with a different `UserFunctionKind` tag.  Used by
    /// `@classmethod` / `@staticmethod`: produces a new UserFunction that
    /// shares everything but the kind tag.
    ///
    /// The wrapped function reuses the **original** `id` so the fn_cache and
    /// any other id-keyed caches share a single entry between the decorated
    /// and undecorated forms.  The function body and `is_memo_pure` flag are
    /// identical (the kind tag only affects attribute-lookup-time binding,
    /// not execution), so cache hits across forms are correct.  See #303.
    pub fn with_function_kind(f: Rc<UserFunction>, kind: UserFunctionKind) -> Self {
        // Fast path: kind already matches and is not a wrapper kind — reuse the Rc
        // directly.  Wrapper kinds (StaticMethod/ClassMethod) must always produce a
        // new Rc so that `staticmethod(sm)` gives a fresh object distinct from `sm`
        // (matching CPython identity semantics where each `staticmethod(x)` call
        // returns a new object regardless of whether `x` is already a staticmethod).
        let is_wrapper_kind = matches!(
            kind,
            UserFunctionKind::StaticMethod | UserFunctionKind::ClassMethod
        );
        if f.kind == kind && !is_wrapper_kind {
            return Value::opaque(Opaque::UserFunction(f));
        }
        // When wrapping as staticmethod/classmethod, record `f` directly so
        // `sm.__func__` returns the exact same Rc that was passed in, preserving
        // object identity (`sm.__func__ is f`).
        let wrapped_func = if is_wrapper_kind {
            Some(Rc::clone(&f))
        } else {
            None
        };
        let new_fn = UserFunction {
            id: f.id,
            kind,
            name: f.name.clone(),
            qualname: f.qualname.clone(),
            name_overrides: RefCell::new(f.name_overrides.borrow().clone()),
            module: RefCell::new(f.module.borrow().clone()),
            doc: RefCell::new(f.doc.borrow().clone()),
            attrs: RefCell::new(f.attrs.borrow().as_ref().map(Rc::clone)),
            annotations: RefCell::new(f.annotations.borrow().clone()),
            // Carry over any per-object defaults override (#2395) so a
            // staticmethod/classmethod wrapper observes the same `__defaults__`
            // / `__kwdefaults__` state as the wrapped function.
            defaults_override: RefCell::new(f.defaults_override.borrow().clone()),
            params: f.params.clone(),
            param_binds: Rc::clone(&f.param_binds),
            memo_positional_parameter_count: f.memo_positional_parameter_count,
            self_bind: f.self_bind,
            local_names: Rc::clone(&f.local_names),
            local_index: Rc::clone(&f.local_index),
            global_names: Rc::clone(&f.global_names),
            nonlocal_names: Rc::clone(&f.nonlocal_names),
            env: Rc::clone(&f.env),
            is_memo_pure: f.is_memo_pure,
            precompiled_code: f.precompiled_code.clone(),
            wrapped_func,
        };
        Value::opaque(Opaque::UserFunction(Rc::new(new_fn)))
    }

    pub fn class_method(f: Rc<UserFunction>) -> Self {
        Value::with_function_kind(f, UserFunctionKind::ClassMethod)
    }

    pub fn static_method(f: Rc<UserFunction>) -> Self {
        Value::with_function_kind(f, UserFunctionKind::StaticMethod)
    }

    pub fn class_bound_method(function: Rc<UserFunction>, class: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::ClassBoundMethod {
            function,
            class,
            obj_id: next_obj_id(),
        })
    }

    pub fn super_proxy(class: Rc<RefCell<PyClass>>, instance: Rc<RefCell<PyInstance>>) -> Self {
        Value::opaque(Opaque::SuperProxy {
            class,
            instance,
            obj_id: next_obj_id(),
        })
    }

    pub fn super_proxy_class(class: Rc<RefCell<PyClass>>, obj_class: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::SuperProxyClass {
            class,
            obj_class,
            obj_id: next_obj_id(),
        })
    }

    /// Construct the unbound super object produced by `super(cls)` (#2704).
    pub fn super_proxy_unbound(class: Rc<RefCell<PyClass>>) -> Self {
        Value::opaque(Opaque::SuperProxyUnbound {
            class,
            obj_id: next_obj_id(),
        })
    }

    /// Create a built-in iterator value (`map`, `zip`, `range_iterator`, a
    /// provider iterator, …).  `state` is the type-erased cursor managed by the
    /// interpreter; these iterators release the cell before running user code,
    /// so their exact type name is read back out of `state`.
    pub fn generator(state: Box<dyn std::any::Any>) -> Self {
        Value::opaque(Opaque::Generator(Rc::new(GeneratorCell {
            kind: GeneratorKind::Iterator,
            names: RefCell::new(None),
            state: RefCell::new(state),
        })))
    }

    /// Create a Python frame object — a generator, coroutine or async
    /// generator.  `state` is the type-erased `GeneratorFrame` managed by the
    /// VM; `kind` and the display names are stored beside it rather than
    /// inside it, because the cell is mutably checked out for as long as the
    /// body runs and the object must still be able to say what it is (#2978).
    pub fn generator_frame(
        state: Box<dyn std::any::Any>,
        kind: GeneratorKind,
        name: std::sync::Arc<str>,
        qualname: std::sync::Arc<str>,
    ) -> Self {
        Value::opaque(Opaque::Generator(Rc::new(GeneratorCell {
            kind,
            names: RefCell::new(Some(GeneratorNames { name, qualname })),
            state: RefCell::new(state),
        })))
    }

    fn opaque(o: Opaque) -> Self {
        // SAFETY: `pool_opaque_alloc` returns a block sized/aligned for
        // `OpaqueSlot`
        // (either a recycled one from this thread's free list or a fresh
        // allocation).  Writing through the cast pointer initialises both the
        // refcount and payload; final `Drop` destroys the slot before returning
        // it to the pool.
        let ptr = unsafe { pool_opaque_alloc() as *mut OpaqueSlot };
        unsafe {
            std::ptr::write(
                ptr,
                OpaqueSlot {
                    strong: Cell::new(1),
                    value: o,
                },
            )
        };
        Value::from_bits(encode_nanbox_heap_pointer(TAG_OPAQUE_BITS, ptr))
    }
}
