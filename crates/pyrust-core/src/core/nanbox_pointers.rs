// ─────────────────────────────────────────────────────────────────────────────
// NaN-boxed heap pointers
// ─────────────────────────────────────────────────────────────────────────────

/// Heap pointers stored in a [`Value`] use the unsigned low 48 bits verbatim.
///
/// Decoding zero-extends those bits back to a machine pointer.  Consequently,
/// this representation supports only non-null pointers whose numeric address
/// is at most [`PAYLOAD_MASK`]; it intentionally does not strip tags or
/// canonicalise/sign-extend a high-half address.  Conventional x86-64 and
/// AArch64 user-space allocators return addresses in this low range.  A target
/// or allocator using wider virtual addresses must fail closed until `Value`'s
/// representation is redesigned.
///
/// All current heap payloads have at least 8-byte alignment.  Besides catching
/// allocator/layout regressions, requiring it preserves the low-bit distinction
/// between heap strings and inline strings.
const NANBOX_HEAP_POINTER_ALIGNMENT: usize = 8;

/// Return the owned-string allocation layout only when every size field can
/// round-trip through the NaN-box string header.
///
/// Layout A stores its byte length in a `u32`. Allowing a larger allocation
/// would truncate the stored length and later deallocate the block with a
/// different layout, which is allocator UB.
#[inline]
fn try_nanbox_owned_string_layout(byte_len: usize) -> Option<Layout> {
    if byte_len > STR_MAX_BYTE_LEN {
        return None;
    }
    let size = STR_OWNED_HEADER_SIZE.checked_add(byte_len)?;
    Layout::from_size_align(size, NANBOX_HEAP_POINTER_ALIGNMENT).ok()
}

#[inline(always)]
fn nanbox_owned_string_layout(byte_len: usize) -> Layout {
    match try_nanbox_owned_string_layout(byte_len) {
        Some(layout) => layout,
        None => abort_unrepresentable_nanbox_string_length(byte_len),
    }
}

/// Allocate a fresh NaN-box payload and apply Rust's standard allocation-error
/// policy before any caller can dereference the returned pointer.
#[inline(always)]
unsafe fn alloc_or_handle(layout: Layout) -> *mut u8 {
    let ptr = unsafe { alloc(layout) };
    if ptr.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    ptr
}

/// Validate the numeric part independently from a real pointer so boundary
/// behaviour can be unit-tested without constructing an invalid pointer.
#[inline(always)]
fn try_nanbox_heap_pointer_payload(address: usize) -> Option<u64> {
    let payload = u64::try_from(address).ok()?;
    (payload != 0
        && payload <= PAYLOAD_MASK
        && address.is_multiple_of(NANBOX_HEAP_POINTER_ALIGNMENT))
    .then_some(payload)
}

/// Encode a heap pointer under `tag_bits`, failing closed if it cannot round
/// trip through the 48-bit payload.
///
/// This check is deliberately active in release builds.  Aborting instead of
/// panicking is important for the `realloc` caller: after a successful realloc,
/// unwinding would drop a `Value` that still contains the now-invalid old
/// pointer.  There is no recoverable representation for the new address while
/// preserving the one-word `Value` ABI, so process termination is the only
/// sound fallback.
#[inline(always)]
fn encode_nanbox_heap_pointer<T>(tag_bits: u64, ptr: *const T) -> u64 {
    debug_assert_eq!(
        tag_bits & PAYLOAD_MASK,
        0,
        "NaN-box pointer tag overlaps its payload"
    );
    let address = ptr.addr();
    match try_nanbox_heap_pointer_payload(address) {
        Some(payload) => tag_bits | payload,
        None => abort_unrepresentable_nanbox_pointer(address),
    }
}

#[cold]
#[inline(never)]
fn abort_unrepresentable_nanbox_pointer(address: usize) -> ! {
    // Do not use `eprintln!`: its internal write unwrap can panic when stderr
    // fails, and this path must never unwind after a successful `realloc`.
    // `write_fmt` exposes the I/O error instead; ignoring it still reaches the
    // unconditional abort.
    let mut stderr = std::io::stderr().lock();
    let _ = std::io::Write::write_fmt(
        &mut stderr,
        format_args!(
            "fatal pyrust-core invariant violation: heap pointer 0x{address:x} cannot round-trip \
             through the 48-bit NaN-box payload\n"
        ),
    );
    std::process::abort()
}

#[cold]
#[inline(never)]
fn abort_unrepresentable_nanbox_string_length(byte_len: usize) -> ! {
    let mut stderr = std::io::stderr().lock();
    let _ = std::io::Write::write_fmt(
        &mut stderr,
        format_args!(
            "fatal pyrust-core invariant violation: string length {byte_len} cannot round-trip \
             through the NaN-box string header\n"
        ),
    );
    std::process::abort()
}
