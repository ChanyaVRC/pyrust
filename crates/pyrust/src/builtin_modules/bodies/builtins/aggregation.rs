use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: sum(iterable, /, start=0) — sum elements of an iterable.
    /// <https://docs.python.org/3/library/functions.html#sum>
    ///
    /// Migrated to the typed-signature dialect (#400): `iterable` is
    /// positional-only `PyValueRef` and `start` is `OptValueRef` so omitted
    /// `start` (0) stays distinct from explicit `sum([], None)` (`None`).
    ///
    /// Mirrors CPython 3.12 `builtin_sum` (#1975 / #2050):
    ///   * iterates the argument lazily — no full materialisation;
    ///   * keeps an `i64` int fast path (drops to generic `__add__` on
    ///     overflow or a big int, exactly like CPython's C-long path);
    ///   * switches to Neumaier (Kahan–Babuška) compensated summation
    ///     the moment a `float` is involved, for bit-exact float sums;
    ///   * falls back to the general `__add__` loop for a non-numeric
    ///     start or the first non-numeric element.
    ///
    /// The accumulator state machine itself is the runtime's
    /// `SumAccumulator`, shared with the compiler-inlined `sum(<genexpr>)`
    /// step (`Insn::SumStep`, #3384).
    #[arity_style(clinic)]
    fn sum(
        #[positional_only] iterable: PyValueRef,
        #[default(OptionalArg::Omitted)] start: OptValueRef,
    ) -> Result<Value> {
        let start = start.as_value();
        // CPython rejects str/bytes/bytearray as the *start* (accumulator)
        // before entering the loop; element-level rejection falls out of the
        // normal `int + str` TypeError in the generic path.
        if let Some(s) = start {
            if s.is_str() {
                return Err(PyError::named(
                    "TypeError",
                    "sum() can't sum strings [use ''.join(seq) instead]",
                ));
            }
            if matches!(s.kind(), ValueKind::Bytes(_)) {
                return Err(PyError::named(
                    "TypeError",
                    "sum() can't sum bytes [use b''.join(seq) instead]",
                ));
            }
            if let ValueKind::BuiltinObject { ops, .. } = s.kind()
                && ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray)
            {
                return Err(PyError::named(
                    "TypeError",
                    "sum() can't sum bytearray [use b''.join(seq) instead]",
                ));
            }
        }

        let mut acc = SumAccumulator::from_start(start);

        let iter = _interp.call_function_expanded(
            Value::builtin_function("iter"),
            &[ExpandedCallArg { name: None, value: iterable.0.clone() }],
        )?;

        loop {
            let item = match _interp.call_next(&iter, None) {
                Ok(item) => item,
                Err(ref e) if crate::interpreter::is_stop_iteration_error(e) => break,
                Err(e) => return Err(e),
            };
            acc.add(_interp, item)?;
        }

        Ok(acc.finish())
    }

    /// CPython: any(iterable) — true if any element is truthy.
    /// <https://docs.python.org/3/library/functions.html#any>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` as
    /// `iterable` so user-defined iterables (PyInstance with `__iter__`)
    /// reach the iter protocol rather than the registry-only path.
    /// Iterates lazily so it short-circuits on the first truthy value
    /// without consuming the rest of the iterator (fixes #1224).
    fn any(#[positional_only] iterable: PyValue) -> Result<Value> {
        let iter = _interp.call_function_expanded(
            Value::builtin_function("iter"),
            &[ExpandedCallArg { name: None, value: iterable.0 }],
        )?;
        loop {
            match _interp.call_next(&iter, None) {
                Ok(item) => {
                    if _interp.truthy_value(&item)? {
                        return Ok(Value::bool_(true));
                    }
                }
                Err(ref e) if crate::interpreter::is_stop_iteration_error(e) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(Value::bool_(false))
    }

    /// CPython: all(iterable) — true if every element is truthy (or empty).
    /// <https://docs.python.org/3/library/functions.html#all>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` as
    /// `iterable` — same rationale as `any`.
    /// Iterates lazily so it short-circuits on the first falsy value
    /// without consuming the rest of the iterator (fixes #1224).
    fn all(#[positional_only] iterable: PyValue) -> Result<Value> {
        let iter = _interp.call_function_expanded(
            Value::builtin_function("iter"),
            &[ExpandedCallArg { name: None, value: iterable.0 }],
        )?;
        loop {
            match _interp.call_next(&iter, None) {
                Ok(item) => {
                    if !_interp.truthy_value(&item)? {
                        return Ok(Value::bool_(false));
                    }
                }
                Err(ref e) if crate::interpreter::is_stop_iteration_error(e) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(Value::bool_(true))
    }
}
