// Helpers
// ---------------------------------------------------------------------------

/// Parse start/end slice args (args[1], args[2]) into byte offsets.
///
/// `end` is clamped to `[0, len]`.  `start` is clamped to `[0, len]` only
/// when it is within bounds; a positive `start > len` is left unclamped so
/// that the `end < start` guard below returns `None`, matching CPython's
/// behaviour of returning -1 / ValueError for an out-of-bounds start.
///
/// Returns `Ok(Some((start, end)))` for a valid (possibly empty) range, or
/// `Ok(None)` when `end < start` — an inverted or out-of-bounds-start range
/// that CPython treats as "no match" (find → -1, count → 0,
/// startswith/endswith → False, index/rindex → ValueError).
fn bytes_slice_args(len: usize, args: &[Value]) -> Result<Option<(usize, usize)>> {
    let start: usize = match args.get(1).map(|v| v.kind()) {
        None | Some(ValueKind::None) => 0,
        Some(ValueKind::Int(i)) => normalise_idx(i, len),
        Some(ValueKind::Bool(b)) => normalise_idx(b as i64, len),
        Some(ValueKind::BigInt(n)) => bigint_start_idx(n, len),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or None or have an __index__ method".to_string(),
            ));
        }
    };
    let end: usize = match args.get(2).map(|v| v.kind()) {
        None | Some(ValueKind::None) => len,
        Some(ValueKind::Int(i)) => normalise_idx(i, len).min(len),
        Some(ValueKind::Bool(b)) => normalise_idx(b as i64, len).min(len),
        Some(ValueKind::BigInt(n)) => bigint_end_idx(n, len),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or None or have an __index__ method".to_string(),
            ));
        }
    };
    // Do NOT clamp start up to len here.  When the caller passes a positive
    // start > len, leaving it unclamped means end (which IS clamped to len)
    // satisfies end < start, triggering the None path below — the same
    // result CPython returns for an out-of-bounds start.  Clamping start to
    // len would incorrectly turn start=6 into start=5==end and produce a
    // valid empty range (returning 5 for empty-sub rfind instead of -1).
    if end < start {
        Ok(None)
    } else {
        Ok(Some((start.min(len), end)))
    }
}

fn normalise_idx(idx: i64, len: usize) -> usize {
    if idx < 0 {
        let from_end = (-idx) as usize;
        len.saturating_sub(from_end)
    } else {
        idx as usize
    }
}

/// Normalise a `BigInt` `start` bound for a search window. A BigInt never fits in
/// an index range, so CPython clamps it: a negative one to the start (`0`), a
/// positive one to just past the end (`len + 1`) so the inverted-window check in
/// the caller reports "not found" rather than a zero-length window (#2688).
fn bigint_start_idx(n: &pyrust_core::PyBigInt, len: usize) -> usize {
    match n.sign() {
        PyBigIntSign::Minus => 0,
        _ => len + 1,
    }
}

/// Normalise a `BigInt` `end` bound for a search window: a negative one clamps to
/// the start (`0`), a positive one to the end (`len`) (#2688).
fn bigint_end_idx(n: &pyrust_core::PyBigInt, len: usize) -> usize {
    match n.sign() {
        PyBigIntSign::Minus => 0,
        _ => len,
    }
}

/// Naive substring search: find first occurrence of `sub` in `haystack`,
/// returning the starting byte index, or `None` if not found.
pub fn find_subsequence(haystack: &[u8], sub: &[u8]) -> Option<usize> {
    if sub.len() > haystack.len() {
        return None;
    }
    haystack.windows(sub.len()).position(|w| w == sub)
}

/// Naive substring search: find last occurrence of `sub` in `haystack`,
/// returning the starting byte index, or `None` if not found.
pub fn rfind_subsequence(haystack: &[u8], sub: &[u8]) -> Option<usize> {
    if sub.len() > haystack.len() {
        return None;
    }
    haystack.windows(sub.len()).rposition(|w| w == sub)
}

// ---------------------------------------------------------------------------
// replace
// ---------------------------------------------------------------------------
