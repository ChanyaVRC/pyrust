//! Simple context managers (`suppress`, `closing`, `nullcontext`, `chdir`,
//! `redirect_stdout`, `redirect_stderr`) for `contextlib`.

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, lookup_exc_class};
use crate::value::{Value, ValueKind};

use super::coroutine;
use super::support::{
    bind_args, bind_required, call, call_method, expect_self, instance_attr, positional,
};

fn self_attr(args: &[ExpandedCallArg], name: &str, fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    Ok(instance_attr(&inst, name).unwrap_or_else(Value::none))
}

// ── suppress ─────────────────────────────────────────────────────────────────

pub(super) fn suppress_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let exceptions: Vec<Value> = args[1..].iter().map(|a| a.value.clone()).collect();
    inst.borrow_mut()
        .attrs
        .insert("_exceptions", Value::tuple(exceptions));
    Ok(Value::none())
}

/// `issubclass(exctype, classinfo)` through the full protocol.
fn is_subclass(interp: &mut Interpreter, exctype: &Value, classinfo: &Value) -> Result<bool> {
    let outcome = interp.call_function_expanded(
        Value::builtin_function("issubclass"),
        &[positional(exctype.clone()), positional(classinfo.clone())],
    )?;
    interp.truthy_value(&outcome)
}

pub(super) fn suppress_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let exceptions = self_attr(args, "_exceptions", fn_name)?;
    let exctype = args
        .get(1)
        .map(|a| a.value.clone())
        .unwrap_or_else(Value::none);
    let excinst = args
        .get(2)
        .map(|a| a.value.clone())
        .unwrap_or_else(Value::none);
    // Unlike isinstance and issubclass, CPython exception handling only looks at
    // the concrete type hierarchy; suppress provides the simpler issubclass
    // based semantics.
    if exctype.is_none() {
        return Ok(Value::none());
    }
    if is_subclass(interp, &exctype, &exceptions)? {
        return Ok(Value::bool_(true));
    }
    if let Some(group) = lookup_exc_class("BaseExceptionGroup")
        && is_subclass(interp, &exctype, &Value::py_class(group))?
    {
        let split = call_method(interp, &excinst, "split", &[exceptions])?;
        let rest = match split.kind() {
            ValueKind::Tuple(pair) => pair.get(1).cloned().unwrap_or_else(Value::none),
            _ => Value::none(),
        };
        if rest.is_none() {
            return Ok(Value::bool_(true));
        }
        return Err(PyError::raised(rest));
    }
    Ok(Value::bool_(false))
}

// ── closing ──────────────────────────────────────────────────────────────────

pub(super) fn closing_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let thing = bind_required(args, fn_name, &["thing"])?.remove(0);
    inst.borrow_mut().attrs.insert("thing", thing);
    Ok(Value::none())
}

pub(super) fn closing_enter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    self_attr(args, "thing", fn_name)
}

pub(super) fn closing_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let thing = self_attr(args, "thing", fn_name)?;
    call_method(interp, &thing, "close", &[])?;
    Ok(Value::none())
}

// ── nullcontext ──────────────────────────────────────────────────────────────

pub(super) fn nullcontext_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let enter_result = bind_args(args, fn_name, &[], &["enter_result"])?
        .remove(0)
        .unwrap_or_else(Value::none);
    inst.borrow_mut().attrs.insert("enter_result", enter_result);
    Ok(Value::none())
}

pub(super) fn nullcontext_enter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    self_attr(args, "enter_result", fn_name)
}

pub(super) fn nullcontext_aenter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    coroutine::coroutine(move |interp| {
        let receiver = Value::py_instance(inst);
        let result = interp.get_attr(&receiver, "enter_result")?;
        Ok(coroutine::Flow::Return(result))
    })
}

// ── chdir ────────────────────────────────────────────────────────────────────

pub(super) fn chdir_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = bind_required(args, fn_name, &["path"])?.remove(0);
    let mut inst = inst.borrow_mut();
    inst.attrs.insert("path", path);
    inst.attrs.insert("_old_cwd", Value::list(vec![]));
    Ok(Value::none())
}

fn os_call(interp: &mut Interpreter, func: &str, values: &[Value]) -> Result<Value> {
    let os_module = interp.load_module("os")?;
    let callable = interp.get_attr(&os_module, func)?;
    call(interp, &callable, values)
}

pub(super) fn chdir_enter(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let path = self_attr(args, "path", fn_name)?;
    let old_cwd = self_attr(args, "_old_cwd", fn_name)?;
    let cwd = os_call(interp, "getcwd", &[])?;
    old_cwd.list_push(cwd)?;
    os_call(interp, "chdir", &[path])?;
    Ok(Value::none())
}

pub(super) fn chdir_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let old_cwd = self_attr(args, "_old_cwd", fn_name)?;
    let restored = pop_last(&old_cwd)?;
    os_call(interp, "chdir", &[restored])?;
    Ok(Value::none())
}

/// `list.pop()`.
fn pop_last(list: &Value) -> Result<Value> {
    match list.list_len() {
        Some(len) if len > 0 => list.list_pop_at(len - 1),
        _ => Err(PyError::named(
            "IndexError",
            "pop from empty list".to_string(),
        )),
    }
}

// ── redirect_stdout / redirect_stderr ────────────────────────────────────────

pub(super) fn redirect_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let target = bind_required(args, fn_name, &["new_target"])?.remove(0);
    let mut inst = inst.borrow_mut();
    inst.attrs.insert("_new_target", target);
    inst.attrs.insert("_old_targets", Value::list(vec![]));
    Ok(Value::none())
}

/// The name of the `sys` attribute this redirect manages (`self._stream`).
fn stream_name(interp: &mut Interpreter, receiver: &Value) -> Result<String> {
    let stream = interp.get_attr(receiver, "_stream")?;
    match stream.as_str() {
        Some(name) => Ok(name.to_string()),
        None => Err(PyError::named(
            "TypeError",
            format!(
                "attribute name must be string, not '{}'",
                crate::interpreter::value_type_name_str(&stream)
            ),
        )),
    }
}

pub(super) fn redirect_enter(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let receiver = Value::py_instance(expect_self(args, fn_name)?);
    let stream = stream_name(interp, &receiver)?;
    let new_target = self_attr(args, "_new_target", fn_name)?;
    let old_targets = self_attr(args, "_old_targets", fn_name)?;
    let old = interp.current_std_stream(&stream)?;
    old_targets.list_push(old)?;
    interp.set_std_stream(&stream, new_target.clone())?;
    Ok(new_target)
}

pub(super) fn redirect_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let receiver = Value::py_instance(expect_self(args, fn_name)?);
    let stream = stream_name(interp, &receiver)?;
    let old_targets = self_attr(args, "_old_targets", fn_name)?;
    let restored = pop_last(&old_targets)?;
    interp.set_std_stream(&stream, restored)?;
    Ok(Value::none())
}

/// A method that does nothing and returns `None` (`nullcontext.__exit__`).
pub(super) fn noop_method(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let _ = expect_self(args, fn_name)?;
    Ok(Value::none())
}
