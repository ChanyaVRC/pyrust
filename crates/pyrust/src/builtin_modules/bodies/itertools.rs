// `itertools` module — body for the `itertools` entry in
// `pyrust_builtin_modules!`.  Currently exposes `chain` and `islice`;
// the rest of the CPython itertools API (`product`, `combinations`,
// `permutations`, `groupby`, `repeat`, `count`, `cycle`, …) is out of
// scope for this initial landing.
//
// ## Laziness vs CPython
//
// CPython's `itertools.*` return *lazy* iterator objects.  This
// initial implementation eagerly collects results into a `list` —
// callers iterating immediately (`for x in chain(...):` /
// `list(chain(...))`) see identical behaviour, but
// `islice(huge_lazy_source, 10)` will fully drain the source instead
// of stopping after 10 elements.  Promoting to true lazy iterators
// is tracked separately.
//
// Reference: <https://docs.python.org/3/library/itertools.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{iter_values, reject_keyword_args_expanded};
use crate::value::{Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: itertools.chain(*iterables) — concatenate iterables.
    /// <https://docs.python.org/3/library/itertools.html#itertools.chain>
    fn chain(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let mut out: Vec<Value> = Vec::new();
        for a in args {
            out.extend(iter_values(a.value.clone())?);
        }
        Ok(Value::list(out))
    }

    /// CPython: itertools.islice(iterable, stop) /
    /// itertools.islice(iterable, start, stop[, step]) — slice an iterable.
    /// `stop = None` means "no stop" (drain).  `step` defaults to 1 and
    /// must be a positive integer.
    /// <https://docs.python.org/3/library/itertools.html#itertools.islice>
    fn islice(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 4 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes between 2 and 4 arguments",
            )));
        }
        // Parse the (start, stop, step) triple — argument count chooses the form.
        // `None` is accepted in any slot to mean the default.
        let (start, stop, step) = match args.len() {
            1 => return Err(PyError::Runtime(format!("{FN_NAME}() takes at least 2 arguments"))),
            2 => (0i64, slice_arg(FN_NAME, &args[1].value, "stop")?, 1i64),
            3 => (
                slice_arg(FN_NAME, &args[1].value, "start")?.unwrap_or(0),
                slice_arg(FN_NAME, &args[2].value, "stop")?,
                1i64,
            ),
            4 => (
                slice_arg(FN_NAME, &args[1].value, "start")?.unwrap_or(0),
                slice_arg(FN_NAME, &args[2].value, "stop")?,
                slice_arg(FN_NAME, &args[3].value, "step")?.unwrap_or(1),
            ),
            _ => unreachable!(),
        };
        if start < 0 || step <= 0 {
            return Err(PyError::named(
                "ValueError",
                format!(
                    "{FN_NAME}() arguments must be non-negative integers (and step > 0)",
                ),
            ));
        }
        // For `stop = None`, we walk the source to exhaustion (stride-aware).
        let items = iter_values(args[0].value.clone())?;
        let step_us = step as usize;
        let start_us = start as usize;
        let stop_us = stop.map(|s| s.max(0) as usize);
        let mut out = Vec::new();
        // Walk indices `start, start+step, start+2*step, …`.  Bounded by
        // `min(items.len(), stop)` when stop is set.
        let upper = match stop_us {
            Some(s) => s.min(items.len()),
            None => items.len(),
        };
        let mut i = start_us;
        while i < upper {
            out.push(items[i].clone());
            i = match i.checked_add(step_us) {
                Some(n) => n,
                None => break,
            };
        }
        Ok(Value::list(out))
    }
}

/// Extract a non-negative `i64` (or `None`) from an `islice` slot.
/// CPython errors on negative ints; we mirror that one level up so the
/// dispatcher rejects all three slots uniformly.
fn slice_arg(fn_name: &str, v: &Value, slot: &str) -> Result<Option<i64>> {
    match v.kind() {
        ValueKind::None => Ok(None),
        ValueKind::Int(n) => Ok(Some(n)),
        ValueKind::Bool(b) => Ok(Some(b as i64)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() {slot} argument must be an integer or None",
            ),
        )),
    }
}
