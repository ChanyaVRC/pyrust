// `itertools.groupby`, `itertools._grouper`, and shared lookahead cursor helpers.

use std::cell::RefCell;
use std::rc::Rc;

use super::helpers::{
    attr_bool, attr_cloned, attr_i64, expect_self, internal, is_stop_iteration, make_iter,
};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};

/// Apply `key_fn(item)` when present; otherwise the item is its own key.
fn compute_key(interp: &mut Interpreter, key_fn: &Value, item: &Value) -> Result<Value> {
    if key_fn.is_none() {
        Ok(item.clone())
    } else {
        interp.call_function_expanded(
            key_fn.clone(),
            &[ExpandedCallArg {
                name: None,
                value: item.clone(),
            }],
        )
    }
}

/// Compare group keys through Python equality and truth conversion.
fn keys_equal(interp: &mut Interpreter, left: &Value, right: &Value) -> Result<bool> {
    let equal = interp.eval_binary(left.clone(), crate::ast::BinaryOp::Eq, right.clone())?;
    interp.truthy_value(&equal)
}

/// Read `(has_curr, currkey, has_tgt, tgtkey)` from the shared lookahead.
fn read_groupby_curr(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<(bool, Value, bool, Value)> {
    Ok((
        attr_bool(inst, "_has_curr"),
        attr_cloned(inst, "_currkey", fn_name)?,
        attr_bool(inst, "_has_tgt"),
        attr_cloned(inst, "_tgtkey", fn_name)?,
    ))
}

/// Ensure the shared cursor holds one lookahead element. Fetching remains lazy
/// so side-effecting sources advance at the same point as CPython.
fn groupby_ensure_curr(
    interp: &mut Interpreter,
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
) -> Result<bool> {
    if attr_bool(inst, "_has_curr") {
        return Ok(true);
    }
    let iterator = attr_cloned(inst, "_iter", fn_name)?;
    let key_function = attr_cloned(inst, "_keyfn", fn_name)?;
    let item = match interp.call_next(&iterator, None) {
        Ok(value) => value,
        Err(error) if is_stop_iteration(&error) => {
            let mut attrs = inst.borrow_mut();
            attrs.attrs.insert("_has_curr", Value::bool_(false));
            attrs.attrs.insert("_exhausted", Value::bool_(true));
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let key = compute_key(interp, &key_function, &item)?;
    let mut attrs = inst.borrow_mut();
    attrs.attrs.insert("_currvalue", item);
    attrs.attrs.insert("_currkey", key);
    attrs.attrs.insert("_has_curr", Value::bool_(true));
    Ok(true)
}

/// Mark the shared lookahead consumed so the next ensure operation fetches.
fn groupby_clear_curr(inst: &Rc<RefCell<PyInstance>>) {
    inst.borrow_mut()
        .attrs
        .insert("_has_curr", Value::bool_(false));
}

pub(super) fn groupby_init(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut positional: Vec<Value> = Vec::new();
    let mut key_kw: Option<Value> = None;
    for a in &args[1..] {
        match a.name.as_deref() {
            Some("key") => key_kw = Some(a.value.clone()),
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{other}' is an invalid keyword argument for groupby()"),
                ));
            }
            None => positional.push(a.value.clone()),
        }
    }
    if positional.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "groupby() missing required argument 'iterable' (pos 1)".to_string(),
        ));
    }
    if positional.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "groupby() takes at most 2 arguments ({} given)",
                positional.len()
            ),
        ));
    }
    if positional.len() == 2 && key_kw.is_some() {
        return Err(PyError::named(
            "TypeError",
            "groupby() got multiple values for argument 'key'".to_string(),
        ));
    }
    let iter = make_iter(interp, &positional[0])?;
    let key_fn = key_kw
        .or_else(|| positional.get(1).cloned())
        .unwrap_or_else(Value::none);
    let mut a = inst.borrow_mut();
    a.attrs.insert("_iter", iter);
    a.attrs.insert("_keyfn", key_fn);
    a.attrs.insert("_currkey", Value::none());
    a.attrs.insert("_currvalue", Value::none());
    a.attrs.insert("_has_curr", Value::bool_(false));
    a.attrs.insert("_tgtkey", Value::none());
    a.attrs.insert("_has_tgt", Value::bool_(false));
    a.attrs.insert("_id", Value::int(0));
    a.attrs.insert("_exhausted", Value::bool_(false));
    Ok(Value::none())
}

pub(super) fn groupby_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parent = args[0].value.clone();
    let groupby_class = Rc::clone(&inst.borrow().class);
    // Bump the group id: invalidates any grouper handed out for the prior group.
    let new_id = {
        let next = attr_i64(&inst, "_id", fn_name)? + 1;
        inst.borrow_mut().attrs.insert("_id", Value::int(next));
        next
    };
    loop {
        if !groupby_ensure_curr(interp, &inst, fn_name)? {
            return Err(PyError::named("StopIteration", String::new()));
        }
        let (_, currkey, has_tgt, tgtkey) = read_groupby_curr(&inst, fn_name)?;
        if !has_tgt || !keys_equal(interp, &currkey, &tgtkey)? {
            break;
        }
        groupby_clear_curr(&inst);
    }
    let currkey = attr_cloned(&inst, "_currkey", fn_name)?;
    {
        let mut a = inst.borrow_mut();
        a.attrs.insert("_tgtkey", currkey.clone());
        a.attrs.insert("_has_tgt", Value::bool_(true));
    }
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_parent", parent);
    attrs.insert("_tgtkey", currkey.clone());
    attrs.insert("_id", Value::int(new_id));
    let grouper = super::make_grouper_instance(&groupby_class, attrs)?;
    Ok(Value::tuple(vec![currkey, grouper]))
}

pub(super) fn grouper_next(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let parent_val = attr_cloned(&inst, "_parent", fn_name)?;
    let my_tgtkey = attr_cloned(&inst, "_tgtkey", fn_name)?;
    let my_id = attr_i64(&inst, "_id", fn_name)?;
    let ValueKind::PyInstance(parent) = parent_val.kind() else {
        return Err(internal(fn_name));
    };
    let parent_id = attr_i64(parent, "_id", fn_name)?;
    if parent_id != my_id {
        return Err(PyError::named("StopIteration", String::new()));
    }
    if !groupby_ensure_curr(interp, parent, fn_name)? {
        return Err(PyError::named("StopIteration", String::new()));
    }
    let currkey = attr_cloned(parent, "_currkey", fn_name)?;
    if !keys_equal(interp, &currkey, &my_tgtkey)? {
        return Err(PyError::named("StopIteration", String::new()));
    }
    let currvalue = attr_cloned(parent, "_currvalue", fn_name)?;
    groupby_clear_curr(parent);
    Ok(currvalue)
}
