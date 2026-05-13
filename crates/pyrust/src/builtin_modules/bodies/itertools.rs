// `itertools` module — body for the `itertools` entry in
// `pyrust_builtin_modules!`.  Currently exposes `chain` and `islice`;
// the rest of the CPython itertools API (`product`, `combinations`,
// `permutations`, `groupby`, `repeat`, `count`, `cycle`, …) is out of
// scope for this initial landing.
//
// ## Laziness
//
// - **`islice`** is fully lazy: implemented as a class with
//   `__iter__` / `__next__` dunders that drive the source iterator
//   one element at a time via `Interpreter::call_next`.
//   `islice(range(10_000_000), 3)` walks exactly 3 elements from the
//   source — no up-front materialisation.
// - **`chain`** is lazy *across sources* but eager *within* each
//   source — each iterable is materialised when it's reached.  Good
//   enough for the typical pattern `chain(small_a, small_b, …)`;
//   true per-element laziness for chain would mirror islice's
//   class-based pattern and is tracked as a follow-up.
//
// Reference: <https://docs.python.org/3/library/itertools.html>

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::reject_keyword_args_expanded;
use crate::value::{PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;
use std::rc::Rc;

pyrust_module! {
    /// CPython: itertools.chain(*iterables) — concatenate iterables.
    /// Lazy across sources (each is materialised only when reached).
    /// <https://docs.python.org/3/library/itertools.html#itertools.chain>
    fn chain(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let sources: Vec<Value> = args.iter().map(|a| a.value.clone()).collect();
        Ok(pyrust_builtins::iter_helpers::chain(sources))
    }

    /// `itertools.islice` as a class with `__iter__`/`__next__` dunders
    /// — pulls one element from the source iterator per call to
    /// `__next__`, so `islice(huge_source, 3)` walks exactly 3 elements.
    ///
    /// State stored on `self`:
    /// - `_iter`: the source iterator (already advanced past `start` in
    ///   `__init__`).
    /// - `_remaining_stop`: count of elements left until `stop` (None
    ///   = drain to end).
    /// - `_step`: positive int; we skip `step - 1` elements after each
    ///   yielded one.
    class islice {
        /// CPython: islice(iterable, stop) / islice(iterable, start,
        /// stop[, step]) — `None` in any positional slot means the
        /// default (`start=0`, `stop=∞`, `step=1`).
        /// <https://docs.python.org/3/library/itertools.html#itertools.islice>
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.is_empty() || user.len() > 4 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes between 2 and 4 arguments"),
                ));
            }
            let (start, stop, step) = match user.len() {
                1 => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes at least 2 arguments"),
                )),
                2 => (0i64, slice_arg(FN_NAME, &user[1].value, "stop")?, 1i64),
                3 => (
                    slice_arg(FN_NAME, &user[1].value, "start")?.unwrap_or(0),
                    slice_arg(FN_NAME, &user[2].value, "stop")?,
                    1i64,
                ),
                4 => (
                    slice_arg(FN_NAME, &user[1].value, "start")?.unwrap_or(0),
                    slice_arg(FN_NAME, &user[2].value, "stop")?,
                    slice_arg(FN_NAME, &user[3].value, "step")?.unwrap_or(1),
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

            // Convert the iterable to an iterator via the `iter()` builtin
            // so we get a proper cursor object regardless of whether the
            // source was a list, generator, custom __iter__-providing
            // class, etc.
            let iter = _interp.call_function_expanded(
                Value::builtin_function("iter"),
                &[ExpandedCallArg {
                    name: None,
                    value: user[0].value.clone(),
                }],
            )?;

            // Drain `start` elements up front.  CPython's islice does the
            // same thing — the "skip past start" cost is paid eagerly,
            // but only for the start prefix, not the whole source.
            for _ in 0..start {
                match _interp.call_next(iter.clone(), None) {
                    Ok(_) => {}
                    Err(e) if is_stop_iteration(&e) => break,
                    Err(e) => return Err(e),
                }
            }

            // `stop`, if set, becomes a *remaining* count after the
            // start skip.  Simpler than tracking absolute position
            // because StopIteration from the underlying iterator is
            // independent of our bound.
            let remaining: Option<i64> = stop.map(|s| (s - start).max(0));

            let mut a = inst.borrow_mut();
            a.attrs.insert("_iter".to_string(), iter);
            a.attrs.insert(
                "_remaining_stop".to_string(),
                remaining.map_or_else(Value::none, Value::int),
            );
            a.attrs.insert("_step".to_string(), Value::int(step));
            Ok(Value::none())
        }

        /// Iterators are their own `__iter__` — matches CPython.
        fn __iter__(args) -> Result<Value> {
            Ok(args[0].value.clone())
        }

        /// Pull the next element from `_iter`, then skip `step - 1`
        /// elements to set up the following call.  StopIteration from
        /// the underlying source propagates; our own `stop` bound
        /// triggers StopIteration when `_remaining_stop` hits zero.
        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let (iter, remaining, step) = read_state(&inst, FN_NAME)?;

            if let Some(r) = remaining
                && r <= 0
            {
                return Err(PyError::named("StopIteration", String::new()));
            }

            let item = _interp.call_next(iter.clone(), None)?;

            // Step past (step - 1) elements so the next call lands on
            // the right one.  StopIteration here just means the source
            // is exhausted — record that by setting remaining to 0.
            let mut tail_exhausted = false;
            for _ in 0..(step - 1) {
                match _interp.call_next(iter.clone(), None) {
                    Ok(_) => {}
                    Err(e) if is_stop_iteration(&e) => {
                        tail_exhausted = true;
                        break;
                    }
                    Err(e) => return Err(e),
                }
            }

            // Update remaining.  We consumed `step` elements (one
            // yielded + `step - 1` skipped, modulo early exhaustion).
            let new_remaining = remaining.map(|r| if tail_exhausted { 0 } else { r - step });
            inst.borrow_mut().attrs.insert(
                "_remaining_stop".to_string(),
                new_remaining.map_or_else(Value::none, Value::int),
            );

            Ok(item)
        }
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
            format!("{fn_name}() {slot} argument must be an integer or None"),
        )),
    }
}

/// Method-shared `self` extractor — `args[0]` is the instance.
fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<std::cell::RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(&rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

/// Read the (`_iter`, `_remaining_stop`, `_step`) triple back from `self`.
/// `_remaining_stop = None` (the Python value) maps to Rust `None`,
/// meaning "no stop bound".
fn read_state(
    inst: &Rc<std::cell::RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<(Value, Option<i64>, i64)> {
    let attrs = inst.borrow();
    let iter = attrs.attrs.get("_iter").cloned().ok_or_else(|| {
        PyError::Runtime(format!("internal: {fn_name}() missing _iter state"))
    })?;
    let remaining = match attrs.attrs.get("_remaining_stop").map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => Some(n),
        Some(ValueKind::None) | None => None,
        _ => return Err(PyError::Runtime(format!(
            "internal: {fn_name}() _remaining_stop has wrong type",
        ))),
    };
    let step = match attrs.attrs.get("_step").map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        _ => return Err(PyError::Runtime(format!(
            "internal: {fn_name}() _step is missing or wrong type",
        ))),
    };
    Ok((iter, remaining, step))
}

/// Recognise StopIteration in both the plain (`PyError::Named`) and
/// exception-instance (`PyError::Raised`) forms.  pyrust's iterator
/// surface produces the former for built-in iterators and the latter
/// for `__next__` methods that explicitly raise.
fn is_stop_iteration(e: &PyError) -> bool {
    match e {
        PyError::Named(name, _) => name == "StopIteration",
        PyError::Raised(exc) => matches!(
            exc.kind(),
            ValueKind::PyInstance(i) if i.borrow().class.borrow().name == "StopIteration"
        ),
        _ => false,
    }
}
