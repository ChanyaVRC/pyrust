/// A PEP 657 fine-grained caret anchor for a multi-token expression form
/// (issue #2411): `(full_start, prim_start, prim_end, full_end)`, all 0-based
/// char columns within the expression's (single) source line, end-exclusive.
///
/// The formatter underlines `[full_start, full_end)`, drawing `^` under the
/// "primary" sub-range `[prim_start, prim_end)` and `~` under the rest:
///
/// * **Call / attribute / bare name** — `full == prim`, so the whole span is
///   `^` (and, per CPython, omitted when it covers the entire stripped line).
/// * **Binary op** — `prim` is the operator token, the operands get `~`
///   (`(10 + 2) * 3 / 0` → `~~~~~~~~~~~~~^~~`).
/// * **Subscript** — `prim` is the `[...]` part, the object gets `~`
///   (`d['a']` → `~^^^^^`).
///
/// ## Multi-line sentinel (issue #2571)
///
/// A binary expression whose operands straddle physical lines can't be
/// expressed as a single-line column span (the operator / right-operand columns
/// live on a later line than the displayed source line).  In that case the
/// parser records `prim_end == full_end == [`MULTILINE_FULL_END`]` and the
/// formatter clamps the underline to the end of the displayed line, drawing
/// solid `^` from `full_start` — matching CPython 3.12.
pub type CaretSpan = (u32, u32, u32, u32);

/// Sentinel `full_end` (and `prim_end`) marking a multi-line binary-op caret
/// span (issue #2571): the formatter clamps the underline to the end of the
/// displayed source line and draws solid `^` from `full_start`.  `u32::MAX` is
/// safe — no real source line is this long.
pub const MULTILINE_FULL_END: u32 = u32::MAX;
