//! Index, slice, and bytes-like argument normalization for `bytearray`.

use pyrust_core::{PyBigIntSign, PyError, Result, Value, ValueKind};

use super::ByteArrayState;

/// Resolve a slice object's start/stop/step against a sequence of length
/// `len`. Returns `(start, stop, step)` as signed `i64` values matching CPython
/// slice semantics: `stop` is the *exclusive* boundary, and for a backward
/// slice both `start` and `stop` may be `-1` (an empty slice / a walk down to
/// and including index 0). For forward (`step >= 0`) slices both land in
/// `[0, len]`, so step==1 callers can cast them back to `usize`.
pub(super) fn resolve_slice_indices(
    len: i64,
    start: &Value,
    stop: &Value,
    step: &Value,
) -> Result<(i64, i64, i64)> {
    let step_val: i64 = match step.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => b as i64,
        // A BigInt step never fits in an index range: a positive one saturates to
        // i64::MAX (forward walk that takes at most the first element), a negative
        // one to i64::MIN (reverse walk that takes at most the last element).
        ValueKind::BigInt(b) => match b.sign() {
            PyBigIntSign::Minus => i64::MIN,
            _ => i64::MAX,
        },
        _ => 1,
    };
    if step_val == 0 {
        return Err(PyError::named(
            "ValueError",
            "slice step cannot be zero".to_string(),
        ));
    }

    let clamp = |v: i64, lo: i64, hi: i64| v.max(lo).min(hi);

    let (default_start, default_stop) = if step_val > 0 {
        (0i64, len)
    } else {
        (len - 1, -1i64)
    };

    // Map a BigInt bound to an effective `i64` that lies strictly beyond every
    // clamp boundary, so the shared Int clamping logic below produces the same
    // result CPython does for an out-of-range index: a positive BigInt acts like
    // an index past the end, a negative one like an index before the start.
    let bigint_bound = |b: &pyrust_core::PyBigInt| match b.sign() {
        PyBigIntSign::Minus => -len - 1,
        _ => len + 1,
    };
    // Clamp an already-signed index value the way CPython does, given the slice
    // direction. Negative values are first rebased by `+ len`.
    let clamp_bound = |n: i64| {
        if step_val > 0 {
            if n < 0 {
                clamp(n + len, 0, len)
            } else {
                clamp(n, 0, len)
            }
        } else if n < 0 {
            // Backward slice: a bound below `-len` clamps to -1 (empty);
            // otherwise to at most `len - 1` (the last valid index).
            clamp(n + len, -1, len - 1)
        } else {
            clamp(n, -1, len - 1)
        }
    };

    let start_val: i64 = match start.kind() {
        ValueKind::None => default_start,
        ValueKind::Int(n) => clamp_bound(n),
        ValueKind::Bool(b) => b as i64,
        ValueKind::BigInt(b) => clamp_bound(bigint_bound(b)),
        _ => default_start,
    };
    let stop_val: i64 = match stop.kind() {
        ValueKind::None => default_stop,
        ValueKind::Int(n) => clamp_bound(n),
        ValueKind::Bool(b) => b as i64,
        ValueKind::BigInt(b) => clamp_bound(bigint_bound(b)),
        _ => default_stop,
    };

    // Both `start` and `stop` are kept signed. For a backward slice `start`
    // may be -1 (the slice is empty) and `stop` may be -1 (iterate down to and
    // including index 0); round-tripping either through `usize` would corrupt
    // these boundary cases. For forward slices both land in [0, len], so the
    // step==1 callers can cast back to `usize` safely.
    Ok((start_val, stop_val, step_val))
}

/// Generate index sequence for a slice (start, stop, step) over a sequence.
pub(super) fn slice_indices(start: i64, stop: i64, step: i64) -> impl Iterator<Item = usize> {
    struct SliceIter {
        current: i64,
        stop: i64,
        step: i64,
    }
    impl Iterator for SliceIter {
        type Item = usize;
        fn next(&mut self) -> Option<usize> {
            // Forward (step > 0) and backward (step < 0) slices share the same
            // advance step; only the bound test differs.
            let in_range = (self.step > 0 && self.current < self.stop)
                || (self.step < 0 && self.current > self.stop);
            if in_range {
                let c = self.current as usize;
                // `saturating_add` rather than `+=`: a BigInt step saturates to
                // i64::MIN/MAX, and a non-zero `start` would overflow-panic on the
                // increment after the first element in debug builds. Saturating
                // pins `current` at i64::MAX/MIN, which fails the next `in_range`
                // test (so the slice yields exactly the first element, matching
                // CPython); a *wrapping* add would instead flip the sign and keep
                // `current` in range, yielding a bogus second index. For ordinary
                // steps no saturation ever occurs, so this stays a bare `add` with
                // no perf cost on the common slice walk.
                self.current = self.current.saturating_add(self.step);
                Some(c)
            } else {
                None
            }
        }
    }
    // `stop` is already the exclusive boundary (CPython slice semantics):
    // forward slices stop before `stop`, backward slices stop after `stop`.
    SliceIter {
        current: start,
        stop,
        step,
    }
}

/// Convert a subscript `Value` to a concrete `usize` index into a slice of
/// length `len`.  Raises `IndexError` if out of range.
pub(super) fn value_to_index(key: &Value, len: usize, type_name: &str) -> Result<usize> {
    let idx: i64 = match key.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => b as i64,
        ValueKind::BigInt(_) => {
            return Err(PyError::named(
                "IndexError",
                "cannot fit 'int' into an index-sized integer".to_string(),
            ));
        }
        _ => {
            // CPython 3.12 uses the bare (unquoted) type name here, matching
            // bytes: `bytearray indices must be integers or slices, not float`.
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{type_name} indices must be integers or slices, not {}",
                    pyrust_core::builtin_type_name(key)
                ),
            ));
        }
    };
    let i = if idx < 0 {
        let from_end = idx.unsigned_abs();
        if from_end > len as u64 {
            return Err(PyError::named(
                "IndexError",
                format!("{type_name} index out of range"),
            ));
        }
        len - from_end as usize
    } else {
        let ui = idx as u64;
        if ui >= len as u64 {
            return Err(PyError::named(
                "IndexError",
                format!("{type_name} index out of range"),
            ));
        }
        ui as usize
    };
    Ok(i)
}

/// Convert a `Value` to a single byte (0..255).  Used for item assignment
/// and `append`.
pub(super) fn value_to_byte(v: &Value, _context: &str) -> Result<u8> {
    match v.kind() {
        ValueKind::Int(n) => {
            if (0..=255).contains(&n) {
                Ok(n as u8)
            } else {
                Err(PyError::named(
                    "ValueError",
                    "byte must be in range(0, 256)".to_string(),
                ))
            }
        }
        ValueKind::Bool(b) => Ok(b as u8),
        // A BigInt is a valid int but always outside 0..=255 — CPython raises
        // ValueError, not the "cannot be interpreted as an integer" TypeError
        // used for non-int types.
        ValueKind::BigInt(_) => Err(PyError::named(
            "ValueError",
            "byte must be in range(0, 256)".to_string(),
        )),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(v)
            ),
        )),
    }
}

/// Extract a `Vec<u8>` from a bytes-like value (bytes or bytearray) or an
/// iterable of integers.
pub(super) fn bytes_from_value(v: &Value, context: &str) -> Result<Vec<u8>> {
    match v.kind() {
        ValueKind::Bytes(rc) => Ok(rc.as_slice().to_vec()),
        ValueKind::BuiltinObject { ops, state }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
        {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<ByteArrayState>()
                .expect("bytearray state");
            Ok(s.data.borrow().clone())
        }
        ValueKind::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items.iter() {
                out.push(value_to_byte(item, context)?);
            }
            Ok(out)
        }
        ValueKind::Tuple(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items.iter() {
                out.push(value_to_byte(item, context)?);
            }
            Ok(out)
        }
        // CPython rejects str in slice assignment with a special message;
        // for extend() it iterates the string and fails per-character.
        ValueKind::Str(_) if context != "bytearray.extend" => Err(PyError::named(
            "TypeError",
            "can assign only bytes, buffers, or iterables of ints in range(0, 256)".to_string(),
        )),
        _ => {
            let type_name = pyrust_core::builtin_type_name(v);
            // Try materialising via the registered iter callback.
            let items = pyrust_core::iter_values_via_registry(v).map_err(|_| {
                // Mirror CPython wording for the two call sites:
                // extend() → "can't extend bytearray with <type>"
                // slice assignment → "can assign only bytes, buffers, or iterables of ints in range(0, 256)"
                if context == "bytearray.extend" {
                    PyError::named(
                        "TypeError",
                        format!("can't extend bytearray with {type_name}"),
                    )
                } else {
                    PyError::named(
                        "TypeError",
                        "can assign only bytes, buffers, or iterables of ints in range(0, 256)"
                            .to_string(),
                    )
                }
            })?;
            let mut out = Vec::with_capacity(items.len());
            for item in &items {
                out.push(value_to_byte(item, context)?);
            }
            Ok(out)
        }
    }
}
