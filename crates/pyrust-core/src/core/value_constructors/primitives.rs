// Thread-local justification:
// (a) Why `thread_local!`: Both `EMPTY_TUPLE` (`Value`) and `BUILTIN_FN_CACHE`
//     (`Rc<UserFunction>`) hold `!Send + !Sync` thread-confined runtime objects
//     needed on hot constructor paths without passing an `&Interpreter`.
// (b) Multi-interpreter / multi-threaded impact: Multiple interpreters on one
//     thread share the immutable `()` singleton and interned builtin function
//     stubs (note: mutating attributes on an interned builtin stub is visible
//     on the same thread); separate threads own independent singletons.
crate::gil_local! {
    /// Immutable empty-tuple singleton so `() is ()` is True and empty `*args`
    /// collection avoids heap allocation.
    static EMPTY_TUPLE: Value = {
        let inner = Rc::new(TupleInner {
            items: Vec::new(),
            obj_id: next_obj_id(),
        });
        unsafe { Value::tuple_from_rc(inner) }
    };

    /// Per-GIL-group intern cache mapping static builtin function names to their
    /// shared `Rc<UserFunction>` stub.
    static BUILTIN_FN_CACHE: RefCell<HashMap<&'static str, Rc<UserFunction>>> =
        RefCell::new(HashMap::new());
}

impl Value {
    // ── Primitive & string constructors ──────────────────────────────────────

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
    /// negative.  Preserve the full unsigned value by promoting it to a big
    /// int instead.
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
}
