// `itertools` module — body for the `itertools` entry in
// `pyrust_builtin_modules!`.  Currently exposes `chain` and `islice`;
// the rest of the CPython itertools API (`product`, `combinations`,
// `permutations`, `groupby`, `repeat`, `count`, `cycle`, …) is out of
// scope for this initial landing.
//
// Both `chain` and `islice` return **lazy** iterator objects (backed
// by the `BuiltinObject` types in `pyrust_builtins::iter_helpers`),
// matching CPython.  In particular, `islice(huge_lazy_source, 10)`
// stops after pulling 10 elements; `chain(big_a, big_b)` only walks
// `big_a` until it's exhausted before touching `big_b`.
//
// Reference: <https://docs.python.org/3/library/itertools.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::reject_keyword_args_expanded;
use crate::value::{Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: itertools.chain(*iterables) — concatenate iterables lazily.
    /// <https://docs.python.org/3/library/itertools.html#itertools.chain>
    fn chain(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let sources: Vec<Value> = args.iter().map(|a| a.value.clone()).collect();
        Ok(pyrust_builtins::iter_helpers::chain(sources))
    }

    /// CPython: itertools.islice(iterable, stop) /
    /// itertools.islice(iterable, start, stop[, step]) — slice an iterable.
    ///
    /// `stop = None` means "no stop" (drain to the end).  `step` defaults
    /// to 1 and must be a positive integer; `start` defaults to 0.
    /// Argument validation is performed eagerly so callers see the
    /// canonical CPython error at construction time, not at first
    /// `iter_next()`.
    /// <https://docs.python.org/3/library/itertools.html#itertools.islice>
    fn islice(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() < 2 || args.len() > 4 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes between 2 and 4 arguments",
            )));
        }
        // Parse the (start, stop, step) triple — argument count chooses
        // the form.  `None` in any slot means the default.
        let (start, stop, step) = match args.len() {
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
            _ => unreachable!("guarded above"),
        };
        if start < 0 || step <= 0 || stop.is_some_and(|s| s < 0) {
            return Err(PyError::named(
                "ValueError",
                format!(
                    "{FN_NAME}() arguments must be non-negative integers (and step > 0)",
                ),
            ));
        }
        Ok(pyrust_builtins::iter_helpers::islice(
            args[0].value.clone(),
            start as usize,
            stop.map(|s| s as usize),
            step as usize,
        ))
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
