// Reduction reconstruction helpers shared by shallow and deep copying, plus
// iterator reduction rebuilding.

use super::deep::deep_copy;
use super::memo::memo_insert_obj;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, make_iterator, restore_reduced_iterator_position,
};
use crate::value::Value;

/// Execute a bytearray iterator's `(iter, (carrier,), position)` reduction.
pub(super) fn rebuild_reduced_iterator(
    carrier: Value,
    position: usize,
    interp: &mut Interpreter,
) -> Result<Value> {
    let rebuilt = make_iterator(interp, &carrier)?;
    apply_getitem_reduction_state(&rebuilt, Some(position as i64), interp)?;
    Ok(rebuilt)
}

/// Execute a legacy sequence cursor's typed reduction.
pub(super) fn rebuild_getitem_reduction(
    constructor: Value,
    owner: Value,
    position: Option<i64>,
    interp: &mut Interpreter,
) -> Result<Value> {
    let rebuilt = construct_getitem_reduction(constructor, owner, interp)?;
    apply_getitem_reduction_state(&rebuilt, position, interp)?;
    Ok(rebuilt)
}

pub(super) fn construct_getitem_reduction(
    constructor: Value,
    owner: Value,
    interp: &mut Interpreter,
) -> Result<Value> {
    interp.call_function_expanded(
        constructor,
        &[ExpandedCallArg {
            name: None,
            value: owner,
        }],
    )
}

pub(super) fn apply_getitem_reduction_state(
    rebuilt: &Value,
    position: Option<i64>,
    interp: &mut Interpreter,
) -> Result<()> {
    let Some(position) = position else {
        return Ok(());
    };
    if restore_reduced_iterator_position(rebuilt, position)? {
        return Ok(());
    }
    apply_reduction_state(rebuilt, Value::int(position), interp)
}

fn parse_reduction_tuple(reduction: Value, max_len: usize) -> Result<(Vec<Value>, Vec<Value>)> {
    let parts = reduction
        .as_tuple()
        .map(|parts| parts.to_vec())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "copy reduction must return a string or tuple".to_string(),
            )
        })?;
    if !(2..=max_len).contains(&parts.len()) {
        return Err(PyError::named(
            "TypeError",
            format!("copy reduction has invalid length {}", parts.len()),
        ));
    }
    let args = parts[1]
        .as_tuple()
        .map(|args| args.to_vec())
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "copy reduction args must be a tuple".to_string(),
            )
        })?;
    Ok((parts, args))
}

fn reduction_pair(entry: &Value, interp: &mut Interpreter) -> Result<[Value; 2]> {
    let iterator = make_iterator(interp, entry)?;
    let first = match interp.call_next(&iterator, None) {
        Ok(value) => value,
        Err(error) if crate::interpreter::is_stop_iteration_error(&error) => {
            return Err(pyrust_core::value_err!(
                "not enough values to unpack (expected 2, got 0)"
            ));
        }
        Err(error) => return Err(error),
    };
    let second = match interp.call_next(&iterator, None) {
        Ok(value) => value,
        Err(error) if crate::interpreter::is_stop_iteration_error(&error) => {
            return Err(pyrust_core::value_err!(
                "not enough values to unpack (expected 2, got 1)"
            ));
        }
        Err(error) => return Err(error),
    };
    match interp.call_next(&iterator, None) {
        Err(error) if crate::interpreter::is_stop_iteration_error(&error) => Ok([first, second]),
        Ok(_) => Err(pyrust_core::value_err!(
            "too many values to unpack (expected 2)"
        )),
        Err(error) => Err(error),
    }
}

pub(super) fn reconstruct_reduction_shallow(
    original: &Value,
    reduction: Value,
    interp: &mut Interpreter,
) -> Result<Value> {
    if reduction.as_str().is_some() {
        return Ok(original.clone());
    }
    let (parts, args) = parse_reduction_tuple(reduction, 3)?;
    let call_args: Vec<ExpandedCallArg> = args
        .into_iter()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    let rebuilt = interp.call_function_expanded(parts[0].clone(), &call_args)?;
    if let Some(state) = parts.get(2).filter(|state| !state.is_none()) {
        apply_reduction_state(&rebuilt, state.clone(), interp)?;
    }
    Ok(rebuilt)
}

pub(super) fn reconstruct_reduction_deep(
    original: &Value,
    reduction: Value,
    memo: &Value,
    interp: &mut Interpreter,
) -> Result<Value> {
    if reduction.as_str().is_some() {
        return Ok(original.clone());
    }
    let (parts, args) = parse_reduction_tuple(reduction, 5)?;
    let mut call_args = Vec::with_capacity(args.len());
    for value in args {
        call_args.push(ExpandedCallArg {
            name: None,
            value: deep_copy(value, memo, interp)?,
        });
    }
    let rebuilt = interp.call_function_expanded(parts[0].clone(), &call_args)?;
    memo_insert_obj(memo, original, &rebuilt);
    if let Some(state) = parts.get(2).filter(|state| !state.is_none()) {
        let copied_state = deep_copy(state.clone(), memo, interp)?;
        apply_deep_reduction_state(&rebuilt, copied_state, interp)?;
    }
    if let Some(listiter) = parts.get(3).filter(|listiter| !listiter.is_none()) {
        let iterator = make_iterator(interp, listiter)?;
        loop {
            let item = match interp.call_next(&iterator, None) {
                Ok(item) => item,
                Err(error) if crate::interpreter::is_stop_iteration_error(&error) => break,
                Err(error) => return Err(error),
            };
            let copied_item = deep_copy(item, memo, interp)?;
            let append = interp.get_attr(&rebuilt, "append")?;
            interp.call_function_expanded(
                append,
                &[ExpandedCallArg {
                    name: None,
                    value: copied_item,
                }],
            )?;
        }
    }
    if let Some(dictiter) = parts.get(4).filter(|dictiter| !dictiter.is_none()) {
        let iterator = make_iterator(interp, dictiter)?;
        loop {
            let entry = match interp.call_next(&iterator, None) {
                Ok(entry) => entry,
                Err(error) if crate::interpreter::is_stop_iteration_error(&error) => break,
                Err(error) => return Err(error),
            };
            let [key, value] = reduction_pair(&entry, interp)?;
            let copied_key = deep_copy(key, memo, interp)?;
            let copied_value = deep_copy(value, memo, interp)?;
            let setitem = interp.get_attr(&rebuilt, "__setitem__")?;
            interp.call_function_expanded(
                setitem,
                &[
                    ExpandedCallArg {
                        name: None,
                        value: copied_key,
                    },
                    ExpandedCallArg {
                        name: None,
                        value: copied_value,
                    },
                ],
            )?;
        }
    }
    Ok(rebuilt)
}

fn try_invoke_setstate(rebuilt: &Value, state: &Value, interp: &mut Interpreter) -> Result<bool> {
    match interp.get_attr(rebuilt, "__setstate__") {
        Ok(setstate) => {
            interp.call_function_expanded(
                setstate,
                &[ExpandedCallArg {
                    name: None,
                    value: state.clone(),
                }],
            )?;
            Ok(true)
        }
        Err(error) if error.class_name_is("AttributeError") => Ok(false),
        Err(error) => Err(error),
    }
}

fn update_rebuilt_dict(rebuilt: &Value, dict_state: Value, interp: &mut Interpreter) -> Result<()> {
    let dict = interp.get_attr(rebuilt, "__dict__")?;
    let update = interp.get_attr(&dict, "update")?;
    interp.call_function_expanded(
        update,
        &[ExpandedCallArg {
            name: None,
            value: dict_state,
        }],
    )?;
    Ok(())
}

fn apply_deep_reduction_state(
    rebuilt: &Value,
    state: Value,
    interp: &mut Interpreter,
) -> Result<()> {
    if try_invoke_setstate(rebuilt, &state, interp)? {
        return Ok(());
    }
    let (dict_state, slot_state) = match state.as_tuple() {
        Some(parts) if parts.len() == 2 => (parts[0].clone(), Some(parts[1].clone())),
        _ => (state, None),
    };
    if !dict_state.is_none() {
        update_rebuilt_dict(rebuilt, dict_state, interp)?;
    }
    if let Some(slot_state) = slot_state.filter(|slot_state| !slot_state.is_none()) {
        let items = interp.get_attr(&slot_state, "items")?;
        let entries = interp.call_function_expanded(items, &[])?;
        let iterator = make_iterator(interp, &entries)?;
        loop {
            let entry = match interp.call_next(&iterator, None) {
                Ok(entry) => entry,
                Err(error) if crate::interpreter::is_stop_iteration_error(&error) => break,
                Err(error) => return Err(error),
            };
            let [name, value] = reduction_pair(&entry, interp)?;
            let name = name.as_str().ok_or_else(|| {
                pyrust_core::type_err!(
                    "attribute name must be string, not '{}'",
                    crate::interpreter::full_type_name_str(&name)
                )
            })?;
            interp.assign_attr(rebuilt.clone(), name, value)?;
        }
    }
    Ok(())
}

fn apply_reduction_state(rebuilt: &Value, state: Value, interp: &mut Interpreter) -> Result<()> {
    if try_invoke_setstate(rebuilt, &state, interp)? {
        return Ok(());
    }
    // `copy._reconstruct` uses `hasattr(y, "__setstate__")`, then falls
    // back to `y.__dict__.update(state)`.  Preserve that fallback for an
    // override returning an arbitrary iterator: it intentionally raises
    // TypeError for our integer reduction state on a normal instance and
    // AttributeError for a generator, which has no `__dict__`.
    update_rebuilt_dict(rebuilt, state, interp)
}
