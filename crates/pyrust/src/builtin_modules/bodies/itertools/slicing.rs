// Sequence-slicing and structural `itertools` iterators: `chain`, `islice`,
// `tee`, `pairwise`, and `batched`.

use std::cell::RefCell;
use std::rc::Rc;

use super::helpers::{
    attr_bool, attr_cloned, attr_i64, check_not_exhausted, expect_self, internal,
    is_stop_iteration, make_iter,
};
use super::native_iterators;
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{PyInstance, Value, ValueKind};

pub(super) fn chain_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    reject_keyword_args_expanded("chain", user)?;
    let sources: Vec<Value> = user.iter().map(|a| a.value.clone()).collect();
    let source_iter = make_iter(interp, &Value::tuple(sources))?;
    let mut a = inst.borrow_mut();
    a.attrs.insert("_source", source_iter);
    a.attrs.insert("_active", Value::none());
    Ok(Value::none())
}

pub(super) fn chain_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    loop {
        let active = attr_cloned(&inst, "_active", fn_name)?;
        if !active.is_none() {
            match interp.call_next(&active, None) {
                Ok(v) => return Ok(v),
                Err(e) if is_stop_iteration(&e) => {
                    inst.borrow_mut().attrs.insert("_active", Value::none());
                }
                Err(e) => return Err(e),
            }
        }
        let source = attr_cloned(&inst, "_source", fn_name)?;
        if source.is_none() {
            return Err(PyError::named("StopIteration", String::new()));
        }
        let next_iterable = match interp.call_next(&source, None) {
            Ok(v) => v,
            Err(e) => {
                // Release `_source` on exhaustion/error, matching CPython's
                // `Py_CLEAR(lz->source)` so deepcopying an exhausted `chain(gen)` succeeds.
                let mut a = inst.borrow_mut();
                a.attrs.insert("_source", Value::none());
                a.attrs.insert("_active", Value::none());
                return Err(e);
            }
        };
        let iter = make_iter(interp, &next_iterable)?;
        inst.borrow_mut().attrs.insert("_active", iter);
    }
}

pub(super) fn chain_from_iterable(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    let (class, user): (_, &[ExpandedCallArg]) = match args.split_first() {
        Some((receiver, user)) => match receiver.value.kind() {
            ValueKind::PyClass(class) => (Some(Rc::clone(class)), user),
            _ => (crate::interpreter::itertools_chain_class(), args),
        },
        None => (crate::interpreter::itertools_chain_class(), args),
    };
    reject_keyword_args_expanded("chain.from_iterable", user)?;
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "chain.from_iterable() takes exactly one argument ({} given)",
                user.len()
            ),
        ));
    }
    let outer = crate::interpreter::make_iterator(interp, &user[0].value)?;
    Ok(native_iterators::chain_from_outer(class, outer))
}

fn read_islice_state(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<(Value, i64, Option<i64>, i64)> {
    let attrs = inst.borrow();
    let iter = attrs
        .attrs
        .get("_iter")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    let skip = match attrs.attrs.get("_skip").map(|value| value.kind()) {
        Some(ValueKind::Int(value)) => value,
        _ => return Err(internal(fn_name)),
    };
    let remaining = match attrs.attrs.get("_remaining_stop").map(|value| value.kind()) {
        Some(ValueKind::Int(value)) => Some(value),
        Some(ValueKind::None) | None => None,
        _ => return Err(internal(fn_name)),
    };
    let step = match attrs.attrs.get("_step").map(|value| value.kind()) {
        Some(ValueKind::Int(value)) => value,
        _ => return Err(internal(fn_name)),
    };
    Ok((iter, skip, remaining, step))
}

fn slice_arg(interp: &mut Interpreter, value: &Value, slot: &str) -> Result<Option<i64>> {
    // CPython's evaluate_slice_index honors __index__, but itertools.islice
    // clears coercion failures and replaces them with slot-specific ValueErrors.
    let resolved = match value.kind() {
        ValueKind::None => return Ok(None),
        ValueKind::Int(_) | ValueKind::Bool(_) => Ok(value.clone()),
        ValueKind::PyInstance(_) => interp.value_to_index(value, |_| {
            PyError::named("__pyrust_NotIndex__", String::new())
        }),
        _ => Err(PyError::named("__pyrust_NotIndex__", String::new())),
    };
    let indices_message =
        || "Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.".to_string();
    let slot_error = |coerced_negative_in_range: bool| {
        let message = match slot {
            "step" => "Step for islice() must be a positive integer or None.".to_string(),
            "stop" if !coerced_negative_in_range => {
                "Stop argument for islice() must be None or an integer: 0 <= x <= sys.maxsize."
                    .to_string()
            }
            _ => indices_message(),
        };
        PyError::named("ValueError", message)
    };
    match resolved {
        Ok(resolved) => match resolved.kind() {
            ValueKind::Int(number) if number >= 0 => Ok(Some(number)),
            ValueKind::Bool(value) => Ok(Some(value as i64)),
            ValueKind::Int(number) => Err(slot_error(number <= -2)),
            ValueKind::BigInt(_) => Err(slot_error(false)),
            _ => unreachable!("value_to_index guarantees an integer"),
        },
        Err(_) => Err(slot_error(false)),
    }
}

fn exhaust_islice(inst: &Rc<RefCell<PyInstance>>) {
    let mut attrs = inst.borrow_mut();
    attrs.attrs.insert("_exhausted", Value::bool_(true));
    attrs.attrs.insert("_iter", Value::none());
}

pub(super) fn islice_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    reject_keyword_args_expanded("islice", user)?;
    if user.is_empty() || user.len() > 4 {
        let msg = if user.len() > 4 {
            format!("islice expected at most 4 arguments, got {}", user.len())
        } else {
            format!("islice expected at least 2 arguments, got {}", user.len())
        };
        return Err(PyError::named("TypeError", msg));
    }
    let (start, stop, step) = match user.len() {
        1 => {
            return Err(PyError::named(
                "TypeError",
                "islice expected at least 2 arguments, got 1".to_string(),
            ));
        }
        2 => (0i64, slice_arg(interp, &user[1].value, "stop")?, 1i64),
        3 => (
            slice_arg(interp, &user[1].value, "start")?.unwrap_or(0),
            slice_arg(interp, &user[2].value, "stop")?,
            1i64,
        ),
        4 => (
            slice_arg(interp, &user[1].value, "start")?.unwrap_or(0),
            slice_arg(interp, &user[2].value, "stop")?,
            slice_arg(interp, &user[3].value, "step")?.unwrap_or(1),
        ),
        _ => unreachable!("guarded above"),
    };
    if stop.is_some_and(|s| s < 0) || start < 0 {
        return Err(PyError::named(
            "ValueError",
            "Indices for islice() must be None or an integer: 0 <= x <= sys.maxsize.".to_string(),
        ));
    }
    if step <= 0 {
        return Err(PyError::named(
            "ValueError",
            "Step for islice() must be a positive integer or None.".to_string(),
        ));
    }
    let iter = make_iter(interp, &user[0].value)?;
    let remaining: Option<i64> = stop.map(|s| (s - start).max(0));
    let mut a = inst.borrow_mut();
    a.attrs.insert("_iter", iter);
    a.attrs.insert("_skip", Value::int(start));
    a.attrs.insert("_initial_skip", Value::bool_(true));
    a.attrs.insert("_exhausted", Value::bool_(false));
    a.attrs.insert(
        "_remaining_stop",
        remaining.map_or_else(Value::none, Value::int),
    );
    a.attrs.insert("_step", Value::int(step));
    Ok(Value::none())
}

pub(super) fn islice_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    check_not_exhausted(&inst, "_exhausted")?;
    let (iter, mut skip, mut remaining, step) = read_islice_state(&inst, fn_name)?;
    let initial_skip = attr_bool(&inst, "_initial_skip");
    let skip_now = if initial_skip {
        skip
    } else {
        remaining.map_or(skip, |r| skip.min(r))
    };
    let target_skip = skip - skip_now;
    while skip > target_skip {
        match interp.call_next(&iter, None) {
            Ok(_) => {
                skip -= 1;
                if !initial_skip {
                    remaining = remaining.map(|r| r - 1);
                }
            }
            Err(e) => {
                exhaust_islice(&inst);
                return Err(e);
            }
        }
    }
    {
        let mut attrs = inst.borrow_mut();
        attrs.attrs.insert("_skip", Value::int(0));
        attrs.attrs.insert("_initial_skip", Value::bool_(false));
    }
    if let Some(r) = remaining
        && r <= 0
    {
        exhaust_islice(&inst);
        return Err(PyError::named("StopIteration", String::new()));
    }
    let item = match interp.call_next(&iter, None) {
        Ok(item) => item,
        Err(error) => {
            exhaust_islice(&inst);
            return Err(error);
        }
    };
    remaining = remaining.map(|r| r - 1);
    {
        let mut attrs = inst.borrow_mut();
        attrs.attrs.insert("_skip", Value::int(step - 1));
        attrs.attrs.insert(
            "_remaining_stop",
            remaining.map_or_else(Value::none, Value::int),
        );
    }
    Ok(item)
}

pub(super) fn tee(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut positional: Vec<Value> = Vec::new();
    for a in args {
        match a.name.as_deref() {
            Some(_) => {
                return Err(PyError::named(
                    "TypeError",
                    "itertools.tee() takes no keyword arguments".to_string(),
                ));
            }
            None => positional.push(a.value.clone()),
        }
    }
    let got = positional.len();
    if got == 0 {
        return Err(PyError::named(
            "TypeError",
            format!("tee expected at least 1 argument, got {got}"),
        ));
    }
    if got > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("tee expected at most 2 arguments, got {got}"),
        ));
    }
    let n: usize = match positional.get(1).cloned() {
        None => 2,
        Some(v) => {
            let n = interp.value_to_isize(&v, "Python int too large to convert to C ssize_t")?;
            if n < 0 {
                return Err(PyError::named("ValueError", "n must be >= 0".to_string()));
            }
            n as usize
        }
    };
    Ok(Value::tuple(native_iterators::tee_iterators(
        interp,
        &positional[0],
        n,
    )?))
}

pub(super) fn pairwise_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    reject_keyword_args_expanded("pairwise", user)?;
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("pairwise expected 1 argument, got {}", user.len()),
        ));
    }
    let iter = make_iter(interp, &user[0].value)?;
    let mut a = inst.borrow_mut();
    a.attrs.insert("_iter", iter);
    a.attrs.insert("_prev", Value::none());
    a.attrs.insert("_started", Value::bool_(false));
    a.attrs.insert("_exhausted", Value::bool_(false));
    Ok(Value::none())
}

pub(super) fn pairwise_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    check_not_exhausted(&inst, "_exhausted")?;
    let mut prev = attr_cloned(&inst, "_prev", fn_name)?;
    let iter = attr_cloned(&inst, "_iter", fn_name)?;
    let started = attr_bool(&inst, "_started");
    if !started {
        prev = match interp.call_next(&iter, None) {
            Ok(value) => value,
            Err(e) if is_stop_iteration(&e) => {
                inst.borrow_mut()
                    .attrs
                    .insert("_exhausted", Value::bool_(true));
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        let mut attrs = inst.borrow_mut();
        attrs.attrs.insert("_prev", prev.clone());
        attrs.attrs.insert("_started", Value::bool_(true));
    }
    match interp.call_next(&iter, None) {
        Ok(next) => {
            inst.borrow_mut().attrs.insert("_prev", next.clone());
            Ok(Value::tuple(vec![prev, next]))
        }
        Err(e) if is_stop_iteration(&e) => {
            inst.borrow_mut()
                .attrs
                .insert("_exhausted", Value::bool_(true));
            Err(PyError::named("StopIteration", String::new()))
        }
        Err(e) => Err(e),
    }
}

pub(super) fn batched_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    reject_keyword_args_expanded("batched", user)?;
    if user.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            "batched() missing required argument 'n' (pos 2)".to_string(),
        ));
    }
    if user.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("batched() takes at most 2 arguments ({} given)", user.len()),
        ));
    }
    let n = interp.value_to_isize(
        &user[1].value,
        "Python int too large to convert to C ssize_t",
    )?;
    if n < 1 {
        return Err(PyError::named(
            "ValueError",
            "n must be at least one".to_string(),
        ));
    }
    let iter = make_iter(interp, &user[0].value)?;
    let mut a = inst.borrow_mut();
    a.attrs.insert("_iter", iter);
    a.attrs.insert("_n", Value::int(n));
    a.attrs.insert("_exhausted", Value::bool_(false));
    Ok(Value::none())
}

pub(super) fn batched_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    check_not_exhausted(&inst, "_exhausted")?;
    let iter = attr_cloned(&inst, "_iter", fn_name)?;
    let n = attr_i64(&inst, "_n", fn_name)? as usize;
    let mut batch: Vec<Value> = Vec::with_capacity(n);
    for _ in 0..n {
        match interp.call_next(&iter, None) {
            Ok(v) => batch.push(v),
            Err(e) if is_stop_iteration(&e) => {
                inst.borrow_mut()
                    .attrs
                    .insert("_exhausted", Value::bool_(true));
                break;
            }
            Err(e) => return Err(e),
        }
    }
    if batch.is_empty() {
        Err(PyError::named("StopIteration", String::new()))
    } else {
        Ok(Value::tuple(batch))
    }
}
