// Reduction reconstruction helpers shared by shallow and deep copying, plus
// iterator reduction rebuilding.

use std::rc::Rc;

use super::deep::deep_copy;
use super::memo::memo_insert_obj;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, class_is_subclass_of, invoke_class_method, lookup_class_attr,
    make_iterator, primitive_class_by_name, restore_reduced_iterator_position,
};
use crate::value::{Value, ValueKind};

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

/// Single positional-argument call of `callee`.
fn call_one(callee: Value, arg: Value, interp: &mut Interpreter) -> Result<Value> {
    interp.call_function_expanded(
        callee,
        &[ExpandedCallArg {
            name: None,
            value: arg,
        }],
    )
}

/// Decompose a generic `'T' object is not iterable` TypeError into `T`.
fn not_iterable_type_name(error: &PyError) -> Option<String> {
    match error.kind() {
        pyrust_core::PyErrorKind::Named(class_name, message)
            if class_name.as_ref() == "TypeError" =>
        {
            message
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix("' object is not iterable"))
                .map(str::to_owned)
        }
        _ => None,
    }
}

/// CPython 3.12 builds the argument list of `_reconstruct(x, memo, *rv)` with
/// `LIST_EXTEND`, which rewords the generic "not iterable" refusal to
/// `Value after * must be an iterable, not T` — but only when `T` has neither
/// `tp_iter` nor the sequence protocol. A class that defines `__iter__` (even
/// as `None`) or `__getitem__` keeps the generic message.
fn star_list_extend_error(rv: &Value, error: PyError) -> PyError {
    let Some(type_name) = not_iterable_type_name(&error) else {
        return error;
    };
    if let ValueKind::PyInstance(rc) = rv.kind() {
        let class = Rc::clone(&rc.borrow().class);
        if lookup_class_attr(&class, "__iter__").is_some()
            || lookup_class_attr(&class, "__getitem__").is_some()
        {
            return error;
        }
    }
    pyrust_core::type_err!("Value after * must be an iterable, not {type_name}")
}

/// Mirror CPython's two-item unpack (`key, value = entry`).
fn reduction_pair(entry: &Value, interp: &mut Interpreter) -> Result<[Value; 2]> {
    let iterator =
        make_iterator(interp, entry).map_err(|error| match not_iterable_type_name(&error) {
            Some(type_name) => {
                pyrust_core::type_err!("cannot unpack non-iterable {type_name} object")
            }
            None => error,
        })?;
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

/// Pull the next item of `iterator`, or `None` once it is exhausted.
fn next_item(iterator: &Value, interp: &mut Interpreter) -> Result<Option<Value>> {
    match interp.call_next(iterator, None) {
        Ok(item) => Ok(Some(item)),
        Err(error) if crate::interpreter::is_stop_iteration_error(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Split the reduction value into at most five positional parts
/// (`func, args, state, listiter, dictiter`) after the `_reconstruct(x, memo,
/// *rv)` call, reproducing its arity diagnostics.
fn unpack_reduction(reduction: &Value, interp: &mut Interpreter) -> Result<Vec<Value>> {
    let mut parts = match reduction.as_tuple() {
        Some(parts) => parts.to_vec(),
        None => interp
            .collect_iterable(reduction)
            .map_err(|error| star_list_extend_error(reduction, error))?,
    };
    match parts.len() {
        0 => Err(pyrust_core::type_err!(
            "_reconstruct() missing 2 required positional arguments: 'func' and 'args'"
        )),
        1 => Err(pyrust_core::type_err!(
            "_reconstruct() missing 1 required positional argument: 'args'"
        )),
        given @ 6.. => Err(pyrust_core::type_err!(
            "_reconstruct() takes from 4 to 7 positional arguments but {} were given",
            given + 2
        )),
        _ => {
            parts.resize(5, Value::none());
            Ok(parts)
        }
    }
}

/// `isinstance(value, str)`, including `str` subclass instances.
fn is_str_instance(value: &Value) -> bool {
    if value.as_str().is_some() {
        return true;
    }
    let ValueKind::PyInstance(rc) = value.kind() else {
        return false;
    };
    let class = Rc::clone(&rc.borrow().class);
    primitive_class_by_name("str").is_some_and(|str_class| class_is_subclass_of(&class, &str_class))
}

/// Native `copy._reconstruct(x, memo, func, args, state, listiter, dictiter)`.
///
/// `memo` is `None` for `copy.copy` and the active memo for `copy.deepcopy`.
/// A `str` reduction means "the object is its own copy". Every Python-visible
/// operation (`hasattr`, `__setstate__`, `__dict__.update`, `setattr`,
/// `append`, item assignment) happens in the same order as CPython 3.12.
pub(super) fn reconstruct_reduction(
    original: &Value,
    reduction: Value,
    memo: Option<&Value>,
    interp: &mut Interpreter,
) -> Result<Value> {
    if is_str_instance(&reduction) {
        return Ok(original.clone());
    }
    let parts = unpack_reduction(&reduction, interp)?;
    let [func, args, state, listiter, dictiter] = &parts[..] else {
        unreachable!("unpack_reduction normalises the reduction to five parts");
    };

    let mut call_args: Vec<Value> = Vec::new();
    match memo {
        Some(memo) if interp.truthy_value(args)? => {
            // `args = (deepcopy(arg, memo) for arg in args)`: the iterator is
            // created eagerly and each element is copied as it is consumed.
            let iterator = make_iterator(interp, args)?;
            while let Some(arg) = next_item(&iterator, interp)? {
                call_args.push(deep_copy(arg, memo, interp)?);
            }
        }
        _ => call_args = interp.collect_call_splat(func, args)?,
    }
    let call_args: Vec<ExpandedCallArg> = call_args
        .into_iter()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    let y = interp.call_function_expanded(func.clone(), &call_args)?;
    if let Some(memo) = memo {
        memo_insert_obj(memo, original, &y);
    }

    if !state.is_none() {
        let state = match memo {
            Some(memo) => deep_copy(state.clone(), memo, interp)?,
            None => state.clone(),
        };
        apply_reduction_state(&y, state, interp)?;
    }

    if !listiter.is_none() {
        let iterator = make_iterator(interp, listiter)?;
        while let Some(mut item) = next_item(&iterator, interp)? {
            if let Some(memo) = memo {
                item = deep_copy(item, memo, interp)?;
            }
            let append = interp.get_attr(&y, "append")?;
            call_one(append, item, interp)?;
        }
    }
    if !dictiter.is_none() {
        let iterator = make_iterator(interp, dictiter)?;
        while let Some(entry) = next_item(&iterator, interp)? {
            let [mut key, mut value] = reduction_pair(&entry, interp)?;
            if let Some(memo) = memo {
                key = deep_copy(key, memo, interp)?;
                value = deep_copy(value, memo, interp)?;
            }
            store_item(&y, key, value, interp)?;
        }
    }
    Ok(y)
}

/// `y[key] = value`: the subscript slot is resolved on the type, so an
/// instance attribute named `__setitem__` is not consulted.
fn store_item(target: &Value, key: Value, value: Value, interp: &mut Interpreter) -> Result<()> {
    let args = [
        ExpandedCallArg {
            name: None,
            value: key,
        },
        ExpandedCallArg { name: None, value },
    ];
    if let ValueKind::PyInstance(rc) = target.kind() {
        let class = Rc::clone(&rc.borrow().class);
        if let Some(setitem) = lookup_class_attr(&class, "__setitem__") {
            invoke_class_method(interp, setitem, target.clone(), &args)?;
            return Ok(());
        }
    }
    match interp.get_attr(target, "__setitem__") {
        Ok(setitem) => {
            interp.call_function_expanded(setitem, &args)?;
            Ok(())
        }
        Err(error) if error.class_name_is("AttributeError") => Err(pyrust_core::type_err!(
            "'{}' object does not support item assignment",
            crate::interpreter::value_type_name_str(target)
        )),
        Err(error) => Err(error),
    }
}

/// `hasattr(y, "__setstate__")`: only an `AttributeError` means "absent".
fn has_setstate(target: &Value, interp: &mut Interpreter) -> Result<bool> {
    match interp.get_attr(target, "__setstate__") {
        Ok(_) => Ok(true),
        Err(error) if error.class_name_is("AttributeError") => Ok(false),
        Err(error) => Err(error),
    }
}

/// The `state` stage of `copy._reconstruct`: `y.__setstate__(state)` when
/// available, otherwise `y.__dict__.update(dict_state)` followed by
/// `setattr` for every `(name, value)` of a `(dict_state, slot_state)` pair.
pub(super) fn apply_reduction_state(
    rebuilt: &Value,
    state: Value,
    interp: &mut Interpreter,
) -> Result<()> {
    if has_setstate(rebuilt, interp)? {
        let setstate = interp.get_attr(rebuilt, "__setstate__")?;
        call_one(setstate, state, interp)?;
        return Ok(());
    }
    let (dict_state, slot_state) = match state.as_tuple() {
        Some(parts) if parts.len() == 2 => (parts[0].clone(), parts[1].clone()),
        _ => (state, Value::none()),
    };
    if !dict_state.is_none() {
        let dict = interp.get_attr(rebuilt, "__dict__")?;
        let update = interp.get_attr(&dict, "update")?;
        // The instance `__dict__` proxy only merges real dicts; `dict(state)`
        // supplies CPython's mapping / pair-iterable handling and diagnostics.
        let dict_state = if pyrust_builtins::instance_dict::as_instance_rc(&dict).is_some()
            && !matches!(dict_state.kind(), ValueKind::Dict(_))
            && let Some(dict_class) = primitive_class_by_name("dict")
        {
            call_one(Value::py_class(dict_class), dict_state, interp)?
        } else {
            dict_state
        };
        call_one(update, dict_state, interp)?;
    }
    if !slot_state.is_none() {
        let items = interp.get_attr(&slot_state, "items")?;
        let entries = interp.call_function_expanded(items, &[])?;
        let iterator = make_iterator(interp, &entries)?;
        while let Some(entry) = next_item(&iterator, interp)? {
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
