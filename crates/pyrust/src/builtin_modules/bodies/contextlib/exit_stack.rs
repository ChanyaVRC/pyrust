//! `ExitStack` and `_StackCallback` implementations for `contextlib`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};

use super::helpers::{expect_self, reject_extra_init_args};
use super::registry::make_stack_callback_instance;

pub(super) fn init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    inst.borrow_mut()
        .attrs
        .insert("_callbacks", Value::list(vec![]));
    Ok(Value::none())
}

pub(super) fn enter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    Ok(Value::py_instance(inst))
}

pub(super) fn exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let mut exc_type = args
        .get(1)
        .map(|a| a.value.clone())
        .unwrap_or_else(Value::none);
    let mut exc_val = args
        .get(2)
        .map(|a| a.value.clone())
        .unwrap_or_else(Value::none);
    let mut tb = args
        .get(3)
        .map(|a| a.value.clone())
        .unwrap_or_else(Value::none);
    let cbs = pop_all_callbacks(&inst);
    let mut suppressed = false;
    let mut pending_err: Option<PyError> = None;
    // Walk LIFO (reverse). CPython runs ALL callbacks regardless of exceptions
    // or suppression; only the final state matters.
    for cb in cbs.into_iter().rev() {
        let result = interp.call_function_expanded(
            cb,
            &[
                ExpandedCallArg {
                    name: None,
                    value: exc_type.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: exc_val.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: tb.clone(),
                },
            ],
        );
        match result {
            Ok(v) => {
                // Full interpreter truthiness evaluation handles containers and
                // custom __bool__ / __len__ matching CPython.
                let truthy = match interp.truthy_value(&v) {
                    Ok(b) => b,
                    Err(e) => {
                        pending_err = Some(e);
                        suppressed = false;
                        exc_type = Value::none();
                        exc_val = Value::none();
                        tb = Value::none();
                        continue;
                    }
                };
                if !exc_type.is_none() && truthy {
                    suppressed = true;
                    exc_type = Value::none();
                    exc_val = Value::none();
                    tb = Value::none();
                }
            }
            Err(e) => {
                pending_err = Some(e);
                suppressed = false;
                exc_type = Value::none();
                exc_val = Value::none();
                tb = Value::none();
            }
        }
    }
    if let Some(e) = pending_err {
        return Err(e);
    }
    Ok(Value::bool_(suppressed))
}

pub(super) fn enter_context(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    let cm = user[0].value.clone();
    // Retrieve __exit__ before calling __enter__ so that if __enter__ fails we
    // don't register a cleanup for a context that never opened.
    let exit_fn = interp.get_attr(&cm, "__exit__")?;
    let enter_attr = interp.get_attr(&cm, "__enter__")?;
    let enter_result = interp.call_function_expanded(enter_attr, &[])?;
    let callbacks_val = inst
        .borrow()
        .attrs
        .get("_callbacks")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    callbacks_val.list_push(exit_fn)?;
    Ok(enter_result)
}

pub(super) fn callback(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let exit_stack_class = Rc::clone(&inst.borrow().class);
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes at least 1 argument"),
        ));
    }
    let func = args[1].value.clone();
    let bound_args: Vec<ExpandedCallArg> = args[2..].to_vec();
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_func", func);
    let encoded: Vec<Value> = bound_args
        .iter()
        .map(|a| {
            Value::tuple(vec![
                a.name
                    .as_ref()
                    .map(|n| Value::string(n.clone()))
                    .unwrap_or_else(Value::none),
                a.value.clone(),
            ])
        })
        .collect();
    attrs.insert("_bound_args", Value::list(encoded));
    let wrapper = make_stack_callback_instance(&exit_stack_class, attrs);
    let callbacks_val = inst
        .borrow()
        .attrs
        .get("_callbacks")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    callbacks_val.list_push(wrapper)?;
    Ok(Value::none())
}

pub(super) fn close(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let cbs = pop_all_callbacks(&inst);
    let mut pending_err: Option<PyError> = None;
    for cb in cbs.into_iter().rev() {
        let result = interp.call_function_expanded(
            cb,
            &[
                ExpandedCallArg {
                    name: None,
                    value: Value::none(),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::none(),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::none(),
                },
            ],
        );
        if let Err(e) = result {
            pending_err = Some(e);
        }
    }
    if let Some(e) = pending_err {
        return Err(e);
    }
    Ok(Value::none())
}

pub(super) fn stack_callback_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_extra_init_args(args, fn_name)
}

pub(super) fn stack_callback_call(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let func = inst
        .borrow()
        .attrs
        .get("_func")
        .cloned()
        .ok_or_else(|| PyError::Runtime(format!("internal: {fn_name}() _func missing")))?;
    let encoded_val = inst
        .borrow()
        .attrs
        .get("_bound_args")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    let encoded = match encoded_val.kind() {
        ValueKind::List(items) => items.to_vec(),
        _ => vec![],
    };
    let mut call_args: Vec<ExpandedCallArg> = Vec::new();
    for item in &encoded {
        if let ValueKind::Tuple(pair) = item.kind() {
            let pair = pair.to_vec();
            let name = match pair.first().map(|v| v.kind()) {
                Some(ValueKind::Str(s)) => Some(s.to_string()),
                _ => None,
            };
            let val = pair.get(1).cloned().unwrap_or_else(Value::none);
            call_args.push(ExpandedCallArg { name, value: val });
        }
    }
    interp.call_function_expanded(func, &call_args)?;
    Ok(Value::bool_(false))
}

/// Drain all callbacks from `_callbacks` into a Vec and clear the stack.
fn pop_all_callbacks(inst: &Rc<RefCell<PyInstance>>) -> Vec<Value> {
    let callbacks_val = inst
        .borrow()
        .attrs
        .get("_callbacks")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    // Collect a snapshot first so `list_with` drops its Ref before `list_clear`.
    let result = callbacks_val
        .list_with(|items| items.clone())
        .unwrap_or_default();
    let _ = callbacks_val.list_clear();
    result
}
