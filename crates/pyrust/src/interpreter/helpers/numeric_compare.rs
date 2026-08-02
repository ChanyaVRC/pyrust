/// `a ** b` for non-negative integer exponent, promoting to `BigInt` if the
/// result would overflow `i64`.  Matches CPython's arbitrary-precision int
/// semantics — `2 ** 64` returns the BigInt `18446744073709551616`, not the
/// wrapped value `0`.  The fast path is a single `checked_pow` (≈free), so
/// non-overflowing call sites pay no measurable cost over `wrapping_pow`.
///
/// Centralised here (issue #421 / PR #484 Copilot review) so the `**`
/// operator and the `pow(a, b)` builtin share one source of truth.
pub(crate) fn int_pow_promoting(a: i64, b: i64) -> Value {
    debug_assert!(b >= 0, "int_pow_promoting: caller must guard b < 0");
    let exp = match u32::try_from(b) {
        Ok(e) => e,
        // Exponent doesn't fit in u32 — `a == 0` or `a == 1` or `a == -1` are
        // the only finite-result cases; everything else is astronomically
        // large.  Promote unconditionally; BigInt::pow handles the trivial
        // bases cheaply and produces an honest BigInt for the rest.
        Err(_) => {
            return Value::bigint(PyPow::pow(PyBigInt::from(a), b as u64));
        }
    };
    match a.checked_pow(exp) {
        Some(r) => Value::int(r),
        None => Value::bigint(PyPow::pow(PyBigInt::from(a), exp)),
    }
}
/// Returns the Python type name string for a `Value`, used in error messages.
///
/// Thin alias for [`pyrust_core::builtin_type_name`] — kept locally so the
/// many interpreter call sites stay short.  Returns `Cow<'static, str>` so
/// `PyInstance` can report its runtime class name without a leak; static
/// names stay zero-allocation.
pub(crate) fn value_type_name_str(v: &Value) -> std::borrow::Cow<'static, str> {
    pyrust_core::builtin_type_name(v)
}

/// CPython `tp_name` spelling used only by diagnostics. Unlike
/// [`value_type_name_str`], this may be module-qualified for exact static
/// stdlib classes and must never feed type presentation or runtime policy.
pub(crate) fn error_type_name_str(v: &Value) -> std::borrow::Cow<'static, str> {
    pyrust_core::error_type_name(v)
}

/// Exact ordering between an `i64` integer and an `f64` float.
///
/// Mirrors CPython's richcmp for int vs float: instead of converting the int
/// to f64 (lossy beyond 2^53), we convert the float to its exact integer value
/// and compare there.  Handles all finite and non-finite floats:
///
/// - NaN: returns `None` (caller must treat as unordered; the `compare`
///   wrapper in `expr.rs` already short-circuits NaN to `false`).
/// - `±inf`: ordered relative to every finite i64.
/// - Integer-valued finite float: compare `(f as i64)` to `i`.
/// - Fractional finite float: `i` equals `f.trunc() as i64` only if the
///   fractional part pushes `f` strictly away — positive fraction means
///   `f > i`, negative fraction means `f < i`.
/// - Out-of-i64-range finite float: sign decides ordering.
fn int_float_cmp(i: i64, f: f64) -> Option<std::cmp::Ordering> {
    if f.is_nan() {
        return None;
    }
    // f is ±inf or finite.
    const I64_MAX_PLUS_ONE: f64 = 9_223_372_036_854_775_808.0_f64; // 2^63
    if f >= I64_MAX_PLUS_ONE {
        // float is larger than every i64
        return Some(std::cmp::Ordering::Less);
    }
    if f < (i64::MIN as f64) {
        // float is smaller than every i64
        return Some(std::cmp::Ordering::Greater);
    }
    // f is finite and in [i64::MIN, 2^63); safe to cast.
    let trunc = f.trunc();
    let trunc_i = trunc as i64;
    // base = how i compares to trunc_i (the integer value of f rounded toward zero).
    let base = i.cmp(&trunc_i);
    if base != std::cmp::Ordering::Equal || f == trunc {
        // i != trunc_i: the ordering is unambiguous.
        // i == trunc_i and f is integer-valued: exact equality.
        Some(base)
    } else {
        // i == trunc_i but f has a fractional part: f lies strictly between
        // two integers.  Positive fraction: trunc_i < f < trunc_i+1, so i < f.
        // Negative fraction: trunc_i-1 < f < trunc_i, so i > f.
        if f > 0.0 {
            Some(std::cmp::Ordering::Less)
        } else {
            Some(std::cmp::Ordering::Greater)
        }
    }
}

/// Exact ordering between a `BigInt` and an `f64` float.
///
/// Uses `BigInt::from_f64` (returns `None` for NaN/infinity; for fractional
/// finite floats it truncates toward zero rather than returning `None`) for
/// the integer-valued case — guarded by `f == f.trunc()` so fractional
/// floats fall through to the heuristic path below.  For out-of-range or
/// non-integer floats, falls back to a sign + magnitude heuristic that
/// mirrors CPython's implementation.
fn bigint_float_cmp(big: &crate::value::PyBigInt, f: f64) -> Option<std::cmp::Ordering> {
    use crate::value::PyBigInt;
    use num_traits::FromPrimitive;
    if f.is_nan() {
        return None;
    }
    // For integer-valued finite floats: convert to BigInt and compare exactly.
    if f.is_finite() && f == f.trunc() {
        return PyBigInt::from_f64(f).map(|fi| big.cmp(&fi));
    }
    if f.is_infinite() {
        return if f > 0.0 {
            Some(std::cmp::Ordering::Less) // big < +inf
        } else {
            Some(std::cmp::Ordering::Greater) // big > -inf
        };
    }
    // Fractional finite float: compare big to f.trunc() and adjust.
    let trunc = f.trunc();
    let base = PyBigInt::from_f64(trunc)
        .map(|ti| big.cmp(&ti))
        .unwrap_or(std::cmp::Ordering::Less);
    if base != std::cmp::Ordering::Equal {
        return Some(base);
    }
    // big == trunc but f has a fractional part.
    if f > 0.0 {
        Some(std::cmp::Ordering::Less) // big < f
    } else {
        Some(std::cmp::Ordering::Greater) // big > f
    }
}

/// Total order for Python values used by `sorted()` / `min()` / `max()` and
/// comparison operators.  Mirrors CPython's `<` semantics: numbers by
/// magnitude, strings lexicographically, bools as 0/1, lists and tuples
/// lexicographically element-by-element.  Incomparable pairs return a
/// `TypeError`.
pub(crate) fn compare_values(a: &Value, b: &Value) -> Result<std::cmp::Ordering> {
    compare_values_with_op(a, b, "<")
}

type SequencePairStack = smallvec::SmallVec<[(i64, i64); 8]>;

/// One lexicographic-compare step for a `(List, List)` / `(Tuple, Tuple)`
/// element pair (issue #2216). Mirrors CPython's `list_richcompare`: scan the
/// `==`-equal prefix and only order the first *differing* element.
///
/// The common all-comparable path pays exactly one `compare` dispatch per
/// field (the orderable-Equal arm just `continue`s, incl. `NaN` which orders
/// Equal). The `==` fallback is only consulted on the rare *unorderable* pair
/// (two `None`s) — there it distinguishes an equal-but-unorderable prefix
/// element (`continue`) from a genuine first-differing pair (propagate the
/// `TypeError`). Keeping `==` off the hot path avoids a per-field double
/// dispatch.
macro_rules! seq_prefix_step {
    ($a:expr, $b:expr, $op_name:expr, $active_pairs:expr) => {
        match compare_values_with_op_inner($a, $b, $op_name, $active_pairs) {
            Ok(std::cmp::Ordering::Equal) => continue,
            Ok(ord) => return Ok(ord),
            Err(e) => {
                if matches!(&e, PyError::Named(class, _) if class.as_ref() == "RecursionError") {
                    return Err(e);
                }
                if $a == $b {
                    continue;
                }
                return Err(e);
            }
        }
    };
}

/// Like `compare_values` but uses `op_name` in the `TypeError` message when
/// the operand types are incompatible.  CPython's `do_richcompare` emits the
/// operator token that was actually requested (`<`, `>`, `<=`, `>=`), so
/// `eval_binary` calls this variant directly for `Gt`, `Le`, and `Ge`.
pub(crate) fn compare_values_with_op(
    a: &Value,
    b: &Value,
    op_name: &str,
) -> Result<std::cmp::Ordering> {
    compare_values_with_op_inner(a, b, op_name, &mut SequencePairStack::new())
}

fn compare_values_with_op_inner(
    a: &Value,
    b: &Value,
    op_name: &str,
    active_pairs: &mut SequencePairStack,
) -> Result<std::cmp::Ordering> {
    use crate::value::PyBigInt;
    match (a.kind(), b.kind()) {
        (ValueKind::Int(x), ValueKind::Int(y)) => Ok(x.cmp(&y)),
        (ValueKind::Float(x), ValueKind::Float(y)) => {
            Ok(x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal))
        }
        // Use exact integer comparison to avoid precision loss beyond 2^53.
        // int_float_cmp converts the float to an exact integer value rather
        // than widening the int to f64.  NaN falls through to Equal (the
        // `compare` wrapper in expr.rs pre-filters NaN via `is_nan` checks).
        (ValueKind::Int(x), ValueKind::Float(y)) => {
            Ok(int_float_cmp(x, y).unwrap_or(std::cmp::Ordering::Equal))
        }
        (ValueKind::Float(x), ValueKind::Int(y)) => Ok(int_float_cmp(y, x)
            .map(|o| o.reverse())
            .unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Bool(x), ValueKind::Bool(y)) => Ok(x.cmp(&y)),
        (ValueKind::Bool(x), ValueKind::Int(y)) => Ok((x as i64).cmp(&y)),
        (ValueKind::Int(x), ValueKind::Bool(y)) => Ok(x.cmp(&(y as i64))),
        // bool is a subclass of int, so it must also order against float and
        // BigInt (the plain `<` operator folds bool->int before dispatching;
        // this internal comparator — used by sort/min/max/list ordering — must
        // do the same or `sorted([1.5, True])` raises a spurious TypeError).
        (ValueKind::Bool(x), ValueKind::Float(y)) => {
            Ok(int_float_cmp(x as i64, y).unwrap_or(std::cmp::Ordering::Equal))
        }
        (ValueKind::Float(x), ValueKind::Bool(y)) => Ok(int_float_cmp(y as i64, x)
            .map(|o| o.reverse())
            .unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Bool(x), ValueKind::BigInt(y)) => Ok(PyBigInt::from(x as i64).cmp(y)),
        (ValueKind::BigInt(x), ValueKind::Bool(y)) => Ok((*x).cmp(&PyBigInt::from(y as i64))),
        (ValueKind::BigInt(x), ValueKind::BigInt(y)) => Ok(x.cmp(y)),
        (ValueKind::BigInt(x), ValueKind::Int(y)) => Ok((*x).cmp(&PyBigInt::from(y))),
        (ValueKind::Int(x), ValueKind::BigInt(y)) => Ok(PyBigInt::from(x).cmp(y)),
        (ValueKind::BigInt(x), ValueKind::Float(y)) => {
            Ok(bigint_float_cmp(x, y).unwrap_or(std::cmp::Ordering::Equal))
        }
        (ValueKind::Float(x), ValueKind::BigInt(y)) => Ok(bigint_float_cmp(y, x)
            .map(|o| o.reverse())
            .unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Str(x), ValueKind::Str(y)) => Ok(x.cmp(y)),
        (ValueKind::Bytes(x), ValueKind::Bytes(y)) => Ok(x.as_slice().cmp(y.as_slice())),
        // bytearray <=> bytearray comparison.
        (
            ValueKind::BuiltinObject { ops: aops, .. },
            ValueKind::BuiltinObject { ops: bops, .. },
        ) if aops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray)
            && bops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
        {
            let a_rc = pyrust_builtins::bytearray::as_bytearray_rc(a).expect("bytearray rc");
            let b_rc = pyrust_builtins::bytearray::as_bytearray_rc(b).expect("bytearray rc");
            Ok(a_rc.borrow().as_slice().cmp(b_rc.borrow().as_slice()))
        }
        (ValueKind::List(x), ValueKind::List(y)) => {
            if a.is_identical_to(b) {
                return Ok(std::cmp::Ordering::Equal);
            }
            let pair = (
                a.value_id().expect("list has stable identity"),
                b.value_id().expect("list has stable identity"),
            );
            if active_pairs.contains(&pair) {
                return Err(pyrust_core::py_err!(
                    "RecursionError",
                    "maximum recursion depth exceeded in comparison"
                ));
            }
            active_pairs.push(pair);
            let result = (|| {
                for (a, b) in x.iter().zip(y.iter()) {
                    seq_prefix_step!(a, b, op_name, active_pairs);
                }
                Ok(x.len().cmp(&y.len()))
            })();
            let popped = active_pairs.pop();
            debug_assert_eq!(popped, Some(pair));
            result
        }
        (ValueKind::Tuple(x), ValueKind::Tuple(y)) => {
            if a.is_identical_to(b) {
                return Ok(std::cmp::Ordering::Equal);
            }
            let pair = (
                a.value_id().expect("tuple has stable identity"),
                b.value_id().expect("tuple has stable identity"),
            );
            if active_pairs.contains(&pair) {
                return Err(pyrust_core::py_err!(
                    "RecursionError",
                    "maximum recursion depth exceeded in comparison"
                ));
            }
            active_pairs.push(pair);
            let result = (|| {
                for (a, b) in x.iter().zip(y.iter()) {
                    seq_prefix_step!(a, b, op_name, active_pairs);
                }
                Ok(x.len().cmp(&y.len()))
            })();
            let popped = active_pairs.pop();
            debug_assert_eq!(popped, Some(pair));
            result
        }
        // Two slice objects compare as their `(start, stop, step)` tuples
        // (issue #2127), matching CPython's `slice_richcompare`.  Mixed
        // None/int bounds raise exactly the TypeError the equivalent tuple
        // comparison would (e.g. `slice(None,2) < slice(1,2)` → `None < 1`).
        (
            ValueKind::BuiltinObject { ops: aops, .. },
            ValueKind::BuiltinObject { ops: bops, .. },
        ) if pyrust_builtins::slice::is_slice_ops(aops)
            && pyrust_builtins::slice::is_slice_ops(bops) =>
        {
            let (a_start, a_stop, a_step) =
                pyrust_builtins::slice::slice_fields(a).expect("slice fields");
            let (b_start, b_stop, b_step) =
                pyrust_builtins::slice::slice_fields(b).expect("slice fields");
            for (x, y) in [(&a_start, &b_start), (&a_stop, &b_stop), (&a_step, &b_step)] {
                // Tuple ordering scans the equal prefix with `==` (so equal
                // unorderable fields like two `None`s don't error) and only
                // applies the ordering op to the first *differing* field.
                if x == y {
                    continue;
                }
                return compare_values_with_op_inner(x, y, op_name, active_pairs);
            }
            Ok(std::cmp::Ordering::Equal)
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{op_name}' not supported between instances of '{}' and '{}'",
                error_type_name_str(a),
                error_type_name_str(b),
            ),
        )),
    }
}
