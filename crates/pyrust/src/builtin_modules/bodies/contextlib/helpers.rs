//! Simple context managers (`suppress`, `closing`, `nullcontext`, `chdir`,
//! `redirect_stdout`, `redirect_stderr`) and shared argument helpers for
//! `contextlib`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, class_is_subclass_of};
use crate::value::{PyInstance, Value, ValueKind};

pub(super) fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(rc)),
        _ => Err(PyError::runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

pub(super) fn reject_extra_init_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments"),
        ));
    }
    Ok(Value::none())
}

fn require_one_user_arg(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let user = &args[1..];
    if user.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    Ok(user[0].value.clone())
}

// ── suppress ─────────────────────────────────────────────────────────────────

pub(super) fn suppress_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let types: Vec<Value> = args[1..].iter().map(|a| a.value.clone()).collect();
    inst.borrow_mut().attrs.insert("_types", Value::list(types));
    Ok(Value::none())
}

pub(super) fn suppress_exit(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let exc_type = args
        .get(1)
        .map(|a| a.value.clone())
        .unwrap_or_else(Value::none);
    if exc_type.is_none() {
        return Ok(Value::none());
    }
    let raised_class = match exc_type.kind() {
        ValueKind::PyClass(c) => Rc::clone(c),
        _ => return Ok(Value::bool_(false)),
    };
    let types_val = inst
        .borrow()
        .attrs
        .get("_types")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    let types = match types_val.kind() {
        ValueKind::List(items) => items.to_vec(),
        _ => vec![],
    };
    for t in &types {
        if let ValueKind::PyClass(expected) = t.kind()
            && class_is_subclass_of(&raised_class, expected)
        {
            return Ok(Value::bool_(true));
        }
    }
    Ok(Value::bool_(false))
}

// ── closing ──────────────────────────────────────────────────────────────────

pub(super) fn closing_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let thing = require_one_user_arg(args, fn_name)?;
    inst.borrow_mut().attrs.insert("thing", thing);
    Ok(Value::none())
}

pub(super) fn closing_enter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    Ok(inst
        .borrow()
        .attrs
        .get("thing")
        .cloned()
        .unwrap_or_else(Value::none))
}

pub(super) fn closing_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let thing = inst
        .borrow()
        .attrs
        .get("thing")
        .cloned()
        .unwrap_or_else(Value::none);
    let close_method = interp.get_attr(&thing, "close")?;
    interp.call_function_expanded(close_method, &[])?;
    Ok(Value::none())
}

// ── nullcontext ──────────────────────────────────────────────────────────────

pub(super) fn nullcontext_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let user = &args[1..];
    let enter_result = match user.first() {
        Some(a) if a.name.is_none() || a.name.as_deref() == Some("enter_result") => a.value.clone(),
        None => Value::none(),
        Some(a) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}() got an unexpected keyword argument '{}'",
                    a.name.as_deref().unwrap_or("")
                ),
            ));
        }
    };
    inst.borrow_mut().attrs.insert("enter_result", enter_result);
    Ok(Value::none())
}

pub(super) fn nullcontext_enter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    Ok(inst
        .borrow()
        .attrs
        .get("enter_result")
        .cloned()
        .unwrap_or_else(Value::none))
}

// ── chdir ────────────────────────────────────────────────────────────────────

pub(super) fn chdir_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = require_one_user_arg(args, fn_name)?;
    let mut attrs = inst.borrow_mut();
    attrs.attrs.insert("path", path);
    attrs.attrs.insert("_old_cwd", Value::list(vec![]));
    Ok(Value::none())
}

pub(super) fn chdir_enter(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = inst
        .borrow()
        .attrs
        .get("path")
        .cloned()
        .unwrap_or_else(Value::none);
    let cwd = os_call(interp, "getcwd", &[])?;
    let old_cwd = inst
        .borrow()
        .attrs
        .get("_old_cwd")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    old_cwd.list_push(cwd)?;
    os_call(
        interp,
        "chdir",
        &[ExpandedCallArg {
            name: None,
            value: path,
        }],
    )?;
    Ok(Value::none())
}

pub(super) fn chdir_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let old_cwd = inst
        .borrow()
        .attrs
        .get("_old_cwd")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    let restored = match old_cwd.list_len() {
        Some(n) if n > 0 => old_cwd.list_pop_at(n - 1)?,
        _ => {
            return Err(PyError::named(
                "IndexError",
                "pop from empty list".to_string(),
            ));
        }
    };
    os_call(
        interp,
        "chdir",
        &[ExpandedCallArg {
            name: None,
            value: restored,
        }],
    )?;
    Ok(Value::bool_(false))
}

fn os_call(interp: &mut Interpreter, func: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let os_module = interp.load_module("os")?;
    let callable = interp.get_attr(&os_module, func)?;
    interp.call_function_expanded(callable, args)
}

// ── redirect_stdout / redirect_stderr ────────────────────────────────────────

pub(super) fn redirect_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let target = require_one_user_arg(args, fn_name)?;
    let mut attrs = inst.borrow_mut();
    attrs.attrs.insert("_new_target", target);
    attrs.attrs.insert("_old_targets", Value::list(vec![]));
    Ok(Value::none())
}

pub(super) fn redirect_enter(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    stream: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let new_target = inst
        .borrow()
        .attrs
        .get("_new_target")
        .cloned()
        .unwrap_or_else(Value::none);
    let old = interp.current_std_stream(stream)?;
    let old_targets = inst
        .borrow()
        .attrs
        .get("_old_targets")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    old_targets.list_push(old)?;
    interp.set_std_stream(stream, new_target.clone())?;
    Ok(new_target)
}

pub(super) fn redirect_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    stream: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let old_targets = inst
        .borrow()
        .attrs
        .get("_old_targets")
        .cloned()
        .unwrap_or_else(|| Value::list(vec![]));
    let restored = match old_targets.list_len() {
        Some(n) if n > 0 => old_targets
            .list_pop_at(n - 1)
            .unwrap_or_else(|_| Value::none()),
        _ => Value::none(),
    };
    interp.set_std_stream(stream, restored)?;
    Ok(Value::none())
}

pub(super) fn noop_method(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let _ = expect_self(args, fn_name)?;
    Ok(Value::none())
}
