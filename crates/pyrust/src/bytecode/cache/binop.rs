//! Adaptive binary-operation cache protocol.

/// Operand-type tag for the BinOp inline cache.
///
/// Classifies a `(lhs, rhs)` pair at a BinOp call site into one of four
/// categories.  The cache transitions from `Counting` → `Specialized` (after
/// [`BINOP_SPEC_THRESHOLD`] observations of the same tag) → `Megamorphic`
/// (on a tag mismatch).  A Megamorphic site permanently bypasses the cache
/// and falls straight through to `eval_binary`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinopTypeTag {
    Int,
    Float,
    /// One `int`/`bool` operand and one `float` operand — arithmetic computed by
    /// coercing the int to `f64` (matches CPython's own coercion).
    NumMixed,
    Str,
    Other,
}

/// Adaptive inline-cache state for a single BinOp instruction site.
///
/// Indexed by `pc` inside [`crate::bytecode::FnCode::binop_cache`] — one entry per instruction
/// position.  Only slots for `BinOp` instructions are ever advanced past
/// `Empty`; all other positions remain `Empty` for the lifetime of the
/// `FnCode`.  (`BinOpInPlace`, `BinOpConst`, and `BinOpImm` use only the
/// unconditional int-int fast path and do not consult the adaptive cache.)
#[derive(Debug, Clone, Copy)]
pub(crate) enum BinOpCacheEntry {
    /// No observation yet.
    Empty,
    /// Seen `count` observations all with the same `tag`.
    Counting { tag: BinopTypeTag, count: u8 },
    /// Specialised: every observation so far matched `tag`.
    Specialized(BinopTypeTag),
    /// Two or more distinct tags observed — skip the cache.
    Megamorphic,
}

/// Number of same-type observations required before a BinOp site transitions
/// from `Counting` to `Specialized`.
pub(crate) const BINOP_SPEC_THRESHOLD: u8 = 8;
