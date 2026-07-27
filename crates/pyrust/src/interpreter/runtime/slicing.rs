pub(crate) fn make_slice_value(
    start: Option<Value>,
    stop: Option<Value>,
    step: Option<Value>,
) -> Value {
    pyrust_builtins::slice::make_slice(start, stop, step)
}

impl Interpreter {
    fn slice_index_from_value(value: &Value) -> Result<i64> {
        match value.kind() {
            ValueKind::Int(i) => Ok(i),
            ValueKind::Bool(b) => Ok(if b { 1 } else { 0 }),
            // BigInt slice bounds: clamp to i64 range, matching CPython's behaviour
            // of clamping to sys.maxsize / -sys.maxsize-1 (which on 64-bit platforms
            // equals i64::MAX / i64::MIN).
            ValueKind::BigInt(big) => Ok(match big.to_i64() {
                Some(i) => i,
                None => match big.sign() {
                    PyBigIntSign::Minus => i64::MIN,
                    _ => i64::MAX,
                },
            }),
            _ => Err(pyrust_core::type_err!(
                "slice indices must be integers or None or have an __index__ method"
            )),
        }
    }

    pub(super) fn resolve_slice_bounds(
        len: i64,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
    ) -> Result<(i64, i64, i64)> {
        let step = match st {
            None => 1,
            Some(v) if v.is_none() => 1,
            Some(v) => {
                let s = Self::slice_index_from_value(v)?;
                if s == 0 {
                    return Err(pyrust_core::value_err!("slice step cannot be zero"));
                }
                s
            }
        };

        let normalize = |idx: i64| -> i64 {
            if idx < 0 {
                (idx + len).clamp(0, len)
            } else {
                idx.clamp(0, len)
            }
        };

        let start_default = if step > 0 { 0 } else { len - 1 };
        let end_default = if step > 0 { len } else { -1 };

        let start = match lo {
            None => start_default,
            Some(v) if v.is_none() => start_default,
            Some(v) => {
                let i = Self::slice_index_from_value(v)?;
                if step > 0 {
                    normalize(i)
                } else if i < 0 {
                    (i + len).clamp(-1, len - 1)
                } else {
                    i.clamp(-1, len - 1)
                }
            }
        };

        let end = match hi {
            None => end_default,
            Some(v) if v.is_none() => end_default,
            Some(v) => {
                let i = Self::slice_index_from_value(v)?;
                if step > 0 {
                    normalize(i)
                } else if i < 0 {
                    (i + len).clamp(-1, len - 1)
                } else {
                    i.clamp(-1, len - 1)
                }
            }
        };

        Ok((start, end, step))
    }

    /// Arbitrary-precision `slice.indices(len)` for big-bound range slicing
    /// (#2118).  Mirrors [`Self::resolve_slice_bounds`] but in `BigInt` so a
    /// range whose *length* exceeds i64 (`range(10**20)[:5]`) still slices
    /// correctly instead of overflowing.  `lo`/`hi`/`st` come straight from the
    /// slice object (already `__index__`-resolved to int-like values).
    pub(super) fn resolve_slice_bounds_big(
        len: &PyBigInt,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
    ) -> Result<(PyBigInt, PyBigInt, PyBigInt)> {
        let zero = PyBigInt::from(0);
        let one = PyBigInt::from(1);
        let to_big = |v: &Value| -> Result<PyBigInt> {
            value_to_bigint(v).ok_or_else(|| {
                pyrust_core::type_err!(
                    "slice indices must be integers or None or have an __index__ method"
                )
            })
        };
        let step = match st {
            None => one.clone(),
            Some(v) if v.is_none() => one.clone(),
            Some(v) => {
                let s = to_big(v)?;
                if s == zero {
                    return Err(pyrust_core::value_err!("slice step cannot be zero"));
                }
                s
            }
        };
        let step_pos = step.sign() == PyBigIntSign::Plus;
        // clamp(idx, lo, hi)
        let clamp = |idx: PyBigInt, low: &PyBigInt, high: &PyBigInt| -> PyBigInt {
            if idx < *low {
                low.clone()
            } else if idx > *high {
                high.clone()
            } else {
                idx
            }
        };
        let len_minus_1 = len - &one;
        let resolve = |v: &Value| -> Result<PyBigInt> {
            let i = to_big(v)?;
            let i = if i.sign() == PyBigIntSign::Minus {
                i + len
            } else {
                i
            };
            Ok(if step_pos {
                clamp(i, &zero, len)
            } else {
                // negative step lower bound is -1
                clamp(i, &(-&one), &len_minus_1)
            })
        };
        let start_default = if step_pos {
            zero.clone()
        } else {
            len_minus_1.clone()
        };
        let end_default = if step_pos { len.clone() } else { -&one };
        let start = match lo {
            None => start_default,
            Some(v) if v.is_none() => start_default,
            Some(v) => resolve(v)?,
        };
        let end = match hi {
            None => end_default,
            Some(v) if v.is_none() => end_default,
            Some(v) => resolve(v)?,
        };
        Ok((start, end, step))
    }

    pub(super) fn slice_target_indices(len: i64, start: i64, end: i64, step: i64) -> Vec<usize> {
        // Compute the element count up front (CPython's PySlice_AdjustIndices
        // formula) in `i128` to avoid the overflow that an in-loop `start + step`
        // would hit, then walk a plain countable loop accumulating `i += step`.
        // A BigInt step is clamped to i64::MIN/MAX by `slice_index_from_value`,
        // which yields a count of at most 1; bounding the loop by that count
        // means the accumulator never advances past the last valid index, so the
        // old `wrapping_add` second-index bug cannot occur. Ordinary slices pay
        // no perf cost: the inner loop stays a bare add over a known count.
        let span = (end as i128) - (start as i128);
        let step128 = step as i128;
        let count = if step > 0 {
            if span > 0 {
                ((span - 1) / step128) + 1
            } else {
                0
            }
        } else if span < 0 {
            ((span + 1) / step128) + 1
        } else {
            0
        };
        let count = count.max(0);
        let mut targets = Vec::with_capacity(count as usize);
        let mut i = start;
        // Bounded by the precomputed count, so the accumulator stops on the last
        // valid index and never executes the overflowing `start + step` add that
        // a saturated (BigInt) step would trigger. `last` lets us skip the final,
        // unused increment entirely, keeping the body a bare i64 add.
        let last = count - 1;
        for k in 0..count {
            if i >= 0 && i < len {
                targets.push(i as usize);
            }
            if k != last {
                i += step;
            }
        }
        targets
    }

    /// If `key` is a runtime `slice` object (produced by `BuildSlice`), unpack it.
    /// Returns `Some((lo, hi, step))` where each is `None` for a missing bound.
    ///
    /// Prior to issue #931 this function matched any 3-element tuple, which
    /// ambiguously treated user tuples like `(1, 2, 3)` as slice keys.  The
    /// `BuildSlice` instruction now creates a real slice BuiltinObject, so we
    /// match on that instead.
    pub(crate) fn unpack_slice_key(
        key: &Value,
    ) -> Option<(Option<Value>, Option<Value>, Option<Value>)> {
        if let ValueKind::BuiltinObject { ops, state } = key.kind()
            && pyrust_builtins::slice::is_slice_ops(ops)
        {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<pyrust_builtins::slice::SliceState>()
                .expect("unpack_slice_key: SliceState type mismatch");
            let opt = |v: &Value| if v.is_none() { None } else { Some(v.clone()) };
            return Some((opt(&s.start), opt(&s.stop), opt(&s.step)));
        }
        None
    }

    /// Slice-assign: `items[lo:hi:step] = new_items`.
    pub(crate) fn slice_setitem(
        items: &mut Vec<Value>,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
        new_items: Vec<Value>,
    ) -> Result<()> {
        let len = items.len() as i64;
        let (start, end, step) = Self::resolve_slice_bounds(len, lo, hi, st)?;
        if step == 1 {
            let s = start as usize;
            let e = end as usize;
            items.splice(s..e, new_items);
        } else {
            let indices = Self::slice_target_indices(len, start, end, step);
            if indices.len() != new_items.len() {
                return Err(pyrust_core::value_err!(
                    "attempt to assign sequence of size {} to extended slice of size {}",
                    new_items.len(),
                    indices.len()
                ));
            }
            for (ix, val) in indices.into_iter().zip(new_items) {
                items[ix] = val;
            }
        }
        Ok(())
    }

    /// Slice-delete: `del items[lo:hi:step]` (equivalent to `items[lo:hi:step] = []`).
    pub(crate) fn slice_delitem(
        items: &mut Vec<Value>,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
    ) -> Result<()> {
        let len = items.len() as i64;
        let (start, end, step) = Self::resolve_slice_bounds(len, lo, hi, st)?;
        if step == 1 {
            items.drain(start as usize..end as usize);
            return Ok(());
        }
        let indices = Self::slice_target_indices(len, start, end, step);
        if indices.is_empty() {
            return Ok(());
        }
        // Repeated `Vec::remove` shifts the tail for every target and makes
        // `del values[::2]` quadratic. Mark targets once, then compact in one
        // stable pass.
        let mut deleted = vec![false; items.len()];
        for index in indices {
            deleted[index] = true;
        }
        let mut index = 0;
        items.retain(|_| {
            let keep = !deleted[index];
            index += 1;
            keep
        });
        Ok(())
    }
}
