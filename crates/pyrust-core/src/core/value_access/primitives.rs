impl Value {
    // ── Private unsafe helpers ───────────────────────────────────────────────

    unsafe fn str_hdr(&self) -> *const u8 {
        (self.0 & PAYLOAD_MASK) as *const u8
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
        unsafe { std::ptr::addr_of_mut!((*self.opaque_slot_ptr()).value).cast::<Opaque>() }
    }

    // ── Public primitive accessors ───────────────────────────────────────────

    /// Fast scalar truthiness for NaN-boxed immediate `None`, `bool`, and
    /// inline `int`, using direct bit-checks without decoding [`ValueKind`].
    ///
    /// Returns `None` for floats, strings, containers, bigints, user objects,
    /// and `Value::unset()` so callers fall back to the full truthiness
    /// protocol (and unbound-register diagnostics).
    #[inline(always)]
    pub fn try_scalar_truthiness(&self) -> Option<bool> {
        match top16(self.0) {
            TAG_BOOL => Some((self.0 & 1) != 0),
            TAG_INT => Some(self.0 != TAG_INT_BITS),
            TAG_NONE => Some(false),
            _ => None,
        }
    }

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
}
