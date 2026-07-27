// ── Clone ─────────────────────────────────────────────────────────────────────

impl Clone for Value {
    fn clone(&self) -> Self {
        match top16(self.0) {
            // Primitives: just copy bits
            t if t <= TAG_INT => Value::from_bits(self.0),
            // Str
            TAG_STR => {
                // Inline (SSO, #2832): no heap buffer, no refcount — a clone is
                // just a bit-copy.
                if str_is_inline_bits(self.0) {
                    return Value::from_bits(self.0);
                }
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u32;
                unsafe {
                    // rc is stored in bits 31:3. The helper preserves the
                    // layout/ASCII flags in bits 2:0 and turns a saturated
                    // allocation into an immortal sentinel.
                    str_refcount_increment(hdr);
                } // rc++ (bits 31:3)
                Value::from_bits(self.0) // same bits, 0 allocations
            }
            // Tuple — share the backing `Rc<TupleInner>` with the original; an
            // O(1) strong-count bump rather than a deep copy of the backing
            // `Vec<Value>`.  Tuples are immutable, so sharing is sound and
            // identity (`id()`) is inherent to the shared `TupleInner` (#2268).
            TAG_TUPLE => {
                unsafe {
                    Rc::increment_strong_count(self.tuple_inner_ptr());
                }
                Value::from_bits(self.0)
            }
            // List — share the backing Rc<ListInner> with the original so that
            // mutations through any alias propagate to all clones (#305).  The
            // NaN-box pattern is reused directly; we only bump the strong
            // count to keep the Rc alive.  `obj_id` is inherent to the shared
            // `ListInner`, so identity (`id()`) is automatically stable.
            TAG_LIST => {
                unsafe {
                    Rc::increment_strong_count(self.list_inner_ptr());
                }
                Value::from_bits(self.0)
            }
            // Opaque
            TAG_OPAQUE => {
                let slot = unsafe { &*self.opaque_slot_ptr() };
                let strong = slot.strong.get();
                debug_assert!(strong > 0, "cloning a released opaque slot");
                // Match the string header's saturation policy: an impossibly
                // over-shared value leaks rather than wrapping to zero and
                // freeing a live payload.
                slot.strong.set(strong.saturating_add(1));
                Value::from_bits(self.0)
            }
            _ => unreachable!(),
        }
    }
}

// ── Drop ──────────────────────────────────────────────────────────────────────

impl Drop for Value {
    fn drop(&mut self) {
        match top16(self.0) {
            t if t <= TAG_INT => {} // primitives: no heap
            TAG_STR if str_is_inline_bits(self.0) => {
                // Inline (SSO, #2832): bytes live in the NaN-box, nothing to free.
            }
            TAG_STR => unsafe {
                let hdr = (self.0 & PAYLOAD_MASK) as *mut u8;
                let rc_type_ptr = hdr as *mut u32;
                if str_refcount_decrement(rc_type_ptr) {
                    // rc reached 0
                    if *rc_type_ptr & STR_TYPE_B == 0 {
                        // Layout A owns its lazily allocated Unicode cache.
                        let state = *(hdr.add(8) as *const usize);
                        if state != 0 && state & STR_CODEPOINT_LEN_TAG == 0 {
                            drop(Box::from_raw(state as *mut StrUnicodeCache));
                        }
                        let len = *(hdr.add(4) as *const u32) as usize;
                        dealloc(hdr, nanbox_owned_string_layout(len));
                    } else {
                        // Layout B owns a cache for this slice independently
                        // from its Layout A root.
                        let state = *(hdr.add(STR_SLICE_CACHE_OFFSET) as *const usize);
                        if state != 0 && state & STR_CODEPOINT_LEN_TAG == 0 {
                            drop(Box::from_raw(state as *mut StrUnicodeCache));
                        }
                        // A_ptr = ref - offset - 16
                        let ref_ptr = *(hdr.add(8) as *const *mut u8);
                        let offset = *(hdr.add(16) as *const u32) as usize;
                        let a_ptr = ref_ptr.sub(offset + STR_OWNED_HEADER_SIZE);
                        if str_refcount_decrement(a_ptr as *mut u32) {
                            let root_state = *(a_ptr.add(8) as *const usize);
                            if root_state != 0 && root_state & STR_CODEPOINT_LEN_TAG == 0 {
                                drop(Box::from_raw(root_state as *mut StrUnicodeCache));
                            }
                            let root_len = *(a_ptr.add(4) as *const u32) as usize;
                            dealloc(a_ptr, nanbox_owned_string_layout(root_len));
                        }
                        pool_b_dealloc(hdr);
                    }
                }
            },
            // Tuple — decrement the Rc strong count; the Rc layer drops the
            // underlying `TupleInner` (and its `Vec<Value>`) when the count
            // reaches zero (#2268).
            TAG_TUPLE => unsafe {
                Rc::decrement_strong_count(self.tuple_inner_ptr());
            },
            // List — decrement the Rc strong count; the Rc layer drops the
            // underlying `ListInner` (and its `Vec<Value>`) when the count
            // reaches zero.  Pool allocations from the pre-#305 layout are
            // gone — the Rc-allocated block is freed by the standard
            // allocator, not the pool.
            TAG_LIST => unsafe {
                Rc::decrement_strong_count(self.list_inner_ptr());
            },
            TAG_OPAQUE => unsafe {
                let ptr = self.opaque_slot_ptr();
                let strong = (*ptr).strong.get();
                debug_assert!(strong > 0, "dropping an already released opaque slot");
                if strong == usize::MAX {
                    // Saturated clones are intentionally immortal; see Clone.
                    return;
                }
                if strong > 1 {
                    (*ptr).strong.set(strong - 1);
                    return;
                }
                // Matched allocator: destroy the final payload and hand the
                // whole slot back to the same per-thread pool.
                std::ptr::drop_in_place(ptr);
                pool_opaque_dealloc(ptr as *mut u8);
            },
            _ => unreachable!(),
        }
    }
}
