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
    // Canonical 5-type numeric ordering via pyrust_core::numeric_ops (#458):
    // Int, Bool, Float, BigInt (with NaN -> Equal for total sort stability)
    // and Complex -> TypeError.
    if let Some(res) = pyrust_core::numeric_partial_cmp(a, b, op_name) {
        return res.map(|opt| opt.unwrap_or(std::cmp::Ordering::Equal));
    }
    match (a.kind(), b.kind()) {
        (ValueKind::Str(x), ValueKind::Str(y)) => Ok(x.cmp(y)),
        (ValueKind::Bytes(x), ValueKind::Bytes(y)) => Ok(x.as_slice().cmp(y.as_slice())),
        // bytes <=> bytearray and bytearray <=> bytearray lexicographic
        // ordering (CPython compares both byte-for-byte via bytes_richcompare /
        // bytearray_richcompare, including bytes/bytearray subclasses).
        (ValueKind::Bytes(x), ValueKind::BuiltinObject { ops: bops, .. })
            if bops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
        {
            let b_rc = pyrust_builtins::bytearray::as_bytearray_rc(b).expect("bytearray rc");
            Ok(x.as_slice().cmp(b_rc.borrow().as_slice()))
        }
        (ValueKind::BuiltinObject { ops: aops, .. }, ValueKind::Bytes(y))
            if aops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
        {
            let a_rc = pyrust_builtins::bytearray::as_bytearray_rc(a).expect("bytearray rc");
            Ok(a_rc.borrow().as_slice().cmp(y.as_slice()))
        }
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
