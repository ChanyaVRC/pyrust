/// Python object identity for the runtime's mixed inline/heap value layout.
///
/// `Value` owns the typed identity key and its numeric encoding.  Keeping this
/// interpreter helper as a delegation point preserves the existing call sites
/// while making `is` and `id()` consume one total definition (#2956).
#[inline]
fn values_are_identical(left: &Value, right: &Value) -> bool {
    left.is_identical_to(right)
}
