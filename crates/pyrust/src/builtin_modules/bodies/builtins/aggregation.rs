use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: sum(iterable, /, start=0) — sum elements of an iterable.
    /// <https://docs.python.org/3/library/functions.html#sum>
    ///
    /// Migrated to the typed-signature dialect (#400).  `iterable` is
    /// `PyValue` so user-defined iterables reach the iterator protocol.
    /// `start` is `Option<PyValue>` with `#[default(None)]`; the body
    /// uses 0 when absent.  Known divergence: `sum([], None)` is 0
    /// (not CPython's `None`) because `Option<PyValue>` maps both
    /// "absent" and "Python None" to Rust `None`.  Tracked as a
    /// follow-up fixture under #400.
    ///
    /// Mirrors CPython 3.12 `builtin_sum` (#1975 / #2050):
    ///   * iterates the argument lazily — no full materialisation;
    ///   * keeps an `i64` int fast path (drops to generic `__add__` on
    ///     overflow or a big int, exactly like CPython's C-long path);
    ///   * switches to Neumaier (Kahan–Babuška) compensated summation
    ///     the moment a `float` is involved, for bit-exact float sums;
    ///   * falls back to the general `__add__` loop for a non-numeric
    ///     start or the first non-numeric element.
    fn sum(
        #[positional_only] iterable: PyValue,
        #[positional_only]
        #[default(None)]
        start: Option<PyValue>,
    ) -> Result<Value> {
        let start = start.map(|v| v.0);
        // CPython rejects str/bytes/bytearray as the *start* (accumulator)
        // before entering the loop; element-level rejection falls out of the
        // normal `int + str` TypeError in the generic path.
        if let Some(ref s) = start {
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

        // Accumulator state machine matching CPython's int/float fast paths
        // plus a generic `__add__` fallback.  `Float` carries the running
        // sum and the Neumaier compensation term.  There is deliberately no
        // BigInt fast accumulator: CPython's int fast path uses a C long, and
        // on overflow (or a too-large int item) it drops straight into the
        // generic `PyNumber_Add` loop for the remainder — we mirror that so
        // bit results stay identical when a big int meets a float.
        enum Acc {
            Int(i64),
            Float(f64, f64),
            Generic(Value),
        }

        // Neumaier (Kahan–Babuška) compensated step, bit-identical to
        // CPython 3.12's float fast path.
        fn neumaier(sum: f64, c: f64, x: f64) -> (f64, f64) {
            let t = sum + x;
            let c = if sum.abs() >= x.abs() {
                c + ((sum - t) + x)
            } else {
                c + ((x - t) + sum)
            };
            (t, c)
        }
        // CPython returns `f_result + c`, but drops `c` when it is non-finite
        // so an infinite running sum keeps its sign instead of collapsing to
        // NaN (e.g. `sum([inf, 1.0])` → inf, not nan).
        fn finalize_float(sum: f64, c: f64) -> f64 {
            if c.is_finite() { sum + c } else { sum }
        }

        // Seed the accumulator from `start`.  CPython only enters the int fast
        // path for an *exact* int (`PyLong_CheckExact`): a `bool` start is a
        // subclass, so it (and any non-numeric start) begins in generic mode,
        // which also preserves `sum([], start) == start` unchanged.
        let mut acc = match &start {
            None => Acc::Int(0),
            Some(v) => match v.kind() {
                ValueKind::Int(n) => Acc::Int(n),
                ValueKind::Float(f) => Acc::Float(f, 0.0),
                _ => Acc::Generic(v.clone()),
            },
        };

        let iter = _interp.call_function_expanded(
            Value::builtin_function("iter"),
            &[ExpandedCallArg { name: None, value: iterable.0 }],
        )?;

        loop {
            let item = match _interp.call_next(&iter, None) {
                Ok(item) => item,
                Err(ref e) if crate::interpreter::is_stop_iteration_error(e) => break,
                Err(e) => return Err(e),
            };

            // Classify the element without holding a borrow of `item` (so it
            // can still be moved into the generic fallback).  `bool` items DO
            // participate in the int/float fast paths (CPython's `PyBool_Check`
            // branch); only big ints / non-numerics break out.
            enum Num {
                Int(i64),
                Float(f64),
                Other,
            }
            let num = match item.kind() {
                ValueKind::Int(n) => Num::Int(n),
                ValueKind::Bool(b) => Num::Int(b as i64),
                ValueKind::Float(f) => Num::Float(f),
                _ => Num::Other,
            };

            match &mut acc {
                Acc::Int(s) => match num {
                    Num::Int(n) => match s.checked_add(n) {
                        Some(r) => *s = r,
                        // Overflow: fall to the generic loop with `s + item`,
                        // exactly as CPython does (no BigInt fast path).
                        None => {
                            let cur = Value::int(*s);
                            acc = Acc::Generic(_interp.eval_binary(cur, BinaryOp::Add, item)?);
                        }
                    },
                    // CPython seeds the float path with `f_result =
                    // (double)i_result` then adds the first float plainly
                    // (compensation only kicks in from the *next* element).
                    Num::Float(f) => acc = Acc::Float(*s as f64 + f, 0.0),
                    // Big int (or any non-fast item): continue in generic mode.
                    Num::Other => {
                        let cur = Value::int(*s);
                        acc = Acc::Generic(_interp.eval_binary(cur, BinaryOp::Add, item)?);
                    }
                },
                Acc::Float(s, c) => match num {
                    // Floats go through the compensated step …
                    Num::Float(f) => {
                        let (sum, comp) = neumaier(*s, *c, f);
                        *s = sum;
                        *c = comp;
                    }
                    // … but ints are added plainly (no compensation), exactly
                    // as CPython's float loop handles small `PyLong` items.
                    Num::Int(n) => *s += n as f64,
                    // Big int / non-numeric: hand `f_result + c` to the generic
                    // loop (CPython rebuilds a PyFloat and calls PyNumber_Add).
                    Num::Other => {
                        let cur = Value::float(finalize_float(*s, *c));
                        acc = Acc::Generic(_interp.eval_binary(cur, BinaryOp::Add, item)?);
                    }
                },
                Acc::Generic(s) => {
                    let cur = std::mem::replace(s, Value::int(0));
                    *s = _interp.eval_binary(cur, BinaryOp::Add, item)?;
                }
            }
        }

        Ok(match acc {
            Acc::Int(s) => Value::int(s),
            Acc::Float(s, c) => Value::float(finalize_float(s, c)),
            Acc::Generic(s) => s,
        })
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
