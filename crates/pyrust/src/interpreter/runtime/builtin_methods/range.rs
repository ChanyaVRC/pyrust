// Range construction and O(1) membership semantics.

/// Return whether an i64 value belongs to an i64-backed range.
///
/// The subtraction and remainder run in i128 because two individually valid
/// i64 endpoints can be almost 2**64 apart.  Keeping this arithmetic in i64
/// would either panic in debug builds or wrap and return the wrong membership
/// result in release builds.
#[inline]
pub(super) fn i64_range_contains(start: i64, stop: i64, step: i64, x: i64) -> bool {
    let in_bounds = if step > 0 {
        x >= start && x < stop
    } else if step < 0 {
        x <= start && x > stop
    } else {
        return false;
    };
    in_bounds && ((x as i128 - start as i128) % step as i128 == 0)
}

/// Read a `range()` argument that is already a native `i64` (tagged small int
/// or `bool`).  Anything else — `BigInt`, `__index__` objects, floats — returns
/// `None` so the caller falls back to the general `__index__` path.
#[inline(always)]
fn range_arg_i64(value: &Value) -> Option<i64> {
    match value.kind() {
        ValueKind::Int(n) => Some(n),
        ValueKind::Bool(flag) => Some(i64::from(flag)),
        _ => None,
    }
}

/// Zero-`BigInt` construction for the common all-`i64` argument shapes.
///
/// Returns `None` when any argument is not a plain int/bool (the general path
/// then owns the `__index__` protocol, `BigInt` support and error wording).  A
/// zero step is *not* special-cased here: it falls through to the general path
/// so the `range() arg 3 must not be zero` error has a single source.
#[inline(always)]
fn range_from_i64_args(a: &Value, b: Option<&Value>, c: Option<&Value>) -> Option<Value> {
    let (start, stop, step) = match (b, c) {
        (None, None) => (0, range_arg_i64(a)?, 1),
        (Some(stop), None) => (range_arg_i64(a)?, range_arg_i64(stop)?, 1),
        (Some(stop), Some(step)) => (
            range_arg_i64(a)?,
            range_arg_i64(stop)?,
            range_arg_i64(step)?,
        ),
        (None, Some(_)) => return None,
    };
    if step == 0 {
        return None;
    }
    Some(Value::range(start, stop, step))
}

impl Interpreter {
    pub(crate) fn call_range_values(
        &mut self,
        a: &Value,
        b: Option<&Value>,
        c: Option<&Value>,
    ) -> Result<Value> {
        if let Some(range) = range_from_i64_args(a, b, c) {
            return Ok(range);
        }
        let (start, stop, step) = match (b, c) {
            (None, None) => (
                PyBigInt::from(0),
                self.coerce_range_arg_big(a.clone())?,
                PyBigInt::from(1),
            ),
            (Some(stop_v), None) => (
                self.coerce_range_arg_big(a.clone())?,
                self.coerce_range_arg_big(stop_v.clone())?,
                PyBigInt::from(1),
            ),
            (Some(stop_v), Some(step_v)) => (
                self.coerce_range_arg_big(a.clone())?,
                self.coerce_range_arg_big(stop_v.clone())?,
                self.coerce_range_arg_big(step_v.clone())?,
            ),
            (None, Some(_)) => unreachable!("positional-only range args cannot have a gap"),
        };

        if step == PyBigInt::from(0) {
            return Err(pyrust_core::value_err!("range() arg 3 must not be zero"));
        }

        Ok(Value::range_big(start, stop, step))
    }

    #[allow(dead_code)]
    pub(crate) fn call_range_expanded(&mut self, args: &[ExpandedCallArg]) -> Result<Value> {
        reject_keyword_args_expanded("range", args)?;
        if args.is_empty() {
            return Err(pyrust_core::type_err!(
                "range expected at least 1 argument, got {}",
                args.len()
            ));
        }
        if args.len() > 3 {
            return Err(pyrust_core::type_err!(
                "range expected at most 3 arguments, got {}",
                args.len()
            ));
        }
        self.call_range_values(
            &args[0].value,
            args.get(1).map(|a| &a.value),
            args.get(2).map(|a| &a.value),
        )
    }
    /// Resolve a single `range()` argument to an arbitrary-precision `BigInt`
    /// through the `__index__` protocol (#2118).  Unlike [`Self::coerce_range_arg`]
    /// this does not clamp to `i64`, so `range(10**20)` no longer raises
    /// `OverflowError`.  A non-integer (no `__index__`) raises
    /// `'X' object cannot be interpreted as an integer`, matching CPython 3.12.
    fn coerce_range_arg_big(&mut self, val: Value) -> Result<PyBigInt> {
        let resolved = self.value_to_index(&val, |v| {
            pyrust_core::type_err!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(v)
            )
        })?;
        Ok(value_to_bigint(&resolved).expect("value_to_index returns an int-like value"))
    }
    /// O(1) membership test for range values, matching `eval_in` for Range.
    /// Returns `true` iff `v` is an element of `range(start, stop, step)`.
    pub(super) fn range_contains_value(
        &self,
        start: i64,
        stop: i64,
        step: i64,
        v: &Value,
    ) -> Result<bool> {
        use crate::value::PyToPrimitive;
        Ok(match v.kind() {
            ValueKind::Int(x) => i64_range_contains(start, stop, step, x),
            ValueKind::Bool(b) => i64_range_contains(start, stop, step, b as i64),
            ValueKind::BigInt(n) => n
                .to_i64()
                .is_some_and(|x| i64_range_contains(start, stop, step, x)),
            ValueKind::Float(f) => {
                const I64_MIN_F: f64 = i64::MIN as f64;
                const I64_MAX_PLUS1_F: f64 = 9_223_372_036_854_775_808.0_f64;
                f.is_finite()
                    && f.fract() == 0.0
                    && (I64_MIN_F..I64_MAX_PLUS1_F).contains(&f)
                    && i64_range_contains(start, stop, step, f as i64)
            }
            _ => false,
        })
    }

    /// Arbitrary-precision analogue of [`Self::range_contains_value`] (#2118):
    /// resolves `v` to an integer value (int/bool/bigint or an integer-valued
    /// finite float) and applies the BigInt membership formula.  Returns the
    /// resolved `BigInt` when `v` is a member, else `None`.
    pub(super) fn bigrange_member(
        start: &PyBigInt,
        stop: &PyBigInt,
        step: &PyBigInt,
        v: &Value,
    ) -> Option<PyBigInt> {
        use num_traits::FromPrimitive;
        let x: PyBigInt = match v.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => value_to_bigint(v)?,
            ValueKind::Float(f) if f.is_finite() && f.fract() == 0.0 => PyBigInt::from_f64(f)?,
            _ => return None,
        };
        let sgn = step.sign();
        let in_bounds = if sgn == pyrust_core::PyBigIntSign::Plus {
            x >= *start && x < *stop
        } else {
            x <= *start && x > *stop
        };
        if in_bounds && ((&x - start) % step).sign() == pyrust_core::PyBigIntSign::NoSign {
            Some(x)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod range_membership_tests {
    use super::i64_range_contains;

    #[test]
    fn i64_membership_uses_wide_delta_at_both_boundaries() {
        assert!(i64_range_contains(i64::MIN, i64::MAX, 2, i64::MAX - 1));
        assert!(!i64_range_contains(i64::MIN, i64::MAX, 2, i64::MAX - 2));
        assert!(i64_range_contains(i64::MAX, i64::MIN, -2, i64::MIN + 1));
        assert!(!i64_range_contains(i64::MAX, i64::MIN, -2, i64::MIN + 2));
    }

    #[test]
    fn i64_membership_does_not_use_a_wrapping_delta() {
        // The mathematical deltas are 2**63 + 1 and 2**63 + 2.  Wrapping
        // either to i64 reverses its divisibility by three in release builds.
        assert!(i64_range_contains(i64::MIN, i64::MAX, 3, 1));
        assert!(!i64_range_contains(i64::MIN, i64::MAX, 3, 2));
    }

    #[test]
    fn i64_membership_avoids_min_remainder_by_negative_one() {
        assert!(i64_range_contains(i64::MAX, i64::MIN, -1, -1));
    }
}
