impl Value {
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
