// Infinite / repeating `itertools` iterators: `count`, `repeat`, and `cycle`.

use super::helpers::{
    attr_bool, attr_cloned, expect_self, internal, is_stop_iteration, make_iter, require_numeric,
};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{Value, ValueKind};

pub(super) fn count_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    reject_keyword_args_expanded("count", user)?;
    if user.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("count() takes at most 2 arguments ({} given)", user.len()),
        ));
    }
    let start = user
        .first()
        .cloned()
        .map(|a| a.value)
        .unwrap_or_else(|| Value::int(0));
    let step = user
        .get(1)
        .cloned()
        .map(|a| a.value)
        .unwrap_or_else(|| Value::int(1));
    require_numeric(&start)?;
    require_numeric(&step)?;
    let mut a = inst.borrow_mut();
    a.attrs.insert("_cur", start);
    a.attrs.insert("_step", step);
    Ok(Value::none())
}

pub(super) fn count_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let cur = attr_cloned(&inst, "_cur", fn_name)?;
    let step = attr_cloned(&inst, "_step", fn_name)?;
    let next = interp.eval_binary(cur.clone(), crate::ast::BinaryOp::Add, step)?;
    inst.borrow_mut().attrs.insert("_cur", next);
    Ok(cur)
}

pub(super) fn count_repr(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let cur = attr_cloned(&inst, "_cur", fn_name)?;
    let step = attr_cloned(&inst, "_step", fn_name)?;
    // CPython's count_repr omits the step only when it is an *integer*
    // (incl. bool, a PyLong subclass) equal to 1.  A float `1.0` is
    // always shown.
    let omit_step = matches!(step.kind(), ValueKind::Int(1) | ValueKind::Bool(true));
    if omit_step {
        Ok(Value::string(format!("count({})", cur.repr_raw())))
    } else {
        Ok(Value::string(format!(
            "count({}, {})",
            cur.repr_raw(),
            step.repr_raw()
        )))
    }
}

pub(super) fn repeat_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    reject_keyword_args_expanded("repeat", user)?;
    if user.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "repeat() missing required argument 'object' (pos 1)".to_string(),
        ));
    }
    if user.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("repeat() takes at most 2 arguments ({} given)", user.len()),
        ));
    }
    let object = user[0].value.clone();
    let times: Option<i64> = match user.get(1) {
        None => None,
        Some(a) => {
            Some(interp.value_to_isize(&a.value, "Python int too large to convert to C ssize_t")?)
        }
    };
    let mut a = inst.borrow_mut();
    a.attrs.insert("_object", object);
    a.attrs.insert(
        "_remaining",
        // Negative or zero `times` yields nothing (CPython behaviour).
        times.map_or_else(Value::none, |t| Value::int(t.max(0))),
    );
    Ok(Value::none())
}

pub(super) fn repeat_next(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let object = attr_cloned(&inst, "_object", fn_name)?;
    let remaining = attr_cloned(&inst, "_remaining", fn_name)?;
    match remaining.kind() {
        ValueKind::None => Ok(object),
        ValueKind::Int(n) if n > 0 => {
            inst.borrow_mut()
                .attrs
                .insert("_remaining", Value::int(n - 1));
            Ok(object)
        }
        _ => Err(PyError::named("StopIteration", String::new())),
    }
}

pub(super) fn repeat_repr(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let object = attr_cloned(&inst, "_object", fn_name)?;
    let remaining = attr_cloned(&inst, "_remaining", fn_name)?;
    let obj_repr = crate::interpreter::render_value_repr(interp, &object)?;
    match remaining.kind() {
        ValueKind::Int(n) => Ok(Value::string(format!("repeat({obj_repr}, {n})"))),
        _ => Ok(Value::string(format!("repeat({obj_repr})"))),
    }
}

pub(super) fn cycle_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    reject_keyword_args_expanded("cycle", user)?;
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("cycle expected 1 argument, got {}", user.len()),
        ));
    }
    let iter = make_iter(interp, &user[0].value)?;
    let mut a = inst.borrow_mut();
    a.attrs.insert("_iter", iter);
    a.attrs.insert("_cache", Value::list(Vec::new()));
    a.attrs.insert("_pos", Value::int(0));
    a.attrs.insert("_first_pass", Value::bool_(true));
    Ok(Value::none())
}

pub(super) fn cycle_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    if attr_bool(&inst, "_first_pass") {
        let iter = attr_cloned(&inst, "_iter", fn_name)?;
        match interp.call_next(&iter, None) {
            Ok(v) => {
                let cache = attr_cloned(&inst, "_cache", fn_name)?;
                cache.list_push(v.clone())?;
                return Ok(v);
            }
            Err(e) if is_stop_iteration(&e) => {
                // First pass ended; release `_iter` (matching CPython's
                // `Py_CLEAR(lz->it)` so deepcopying a `cycle(gen)` after the
                // first pass succeeds) and walk `_cache`.
                let mut attrs = inst.borrow_mut();
                attrs.attrs.insert("_first_pass", Value::bool_(false));
                attrs.attrs.insert("_iter", Value::none());
            }
            Err(e) => return Err(e),
        }
    }
    let (item, cache_len, pos) = {
        let attrs = inst.borrow();
        let cache_val = attrs.attrs.get("_cache").ok_or_else(|| internal(fn_name))?;
        let pos = match attrs.attrs.get("_pos").map(|v| v.kind()) {
            Some(ValueKind::Int(n)) => n,
            _ => return Err(internal(fn_name)),
        };
        match cache_val.kind() {
            ValueKind::List(items) if !items.is_empty() => {
                let idx = (pos as usize) % items.len();
                (items[idx].clone(), items.len(), pos)
            }
            ValueKind::List(_) => {
                return Err(PyError::named("StopIteration", String::new()));
            }
            _ => return Err(internal(fn_name)),
        }
    };
    inst.borrow_mut()
        .attrs
        .insert("_pos", Value::int((pos + 1) % cache_len as i64));
    Ok(item)
}
