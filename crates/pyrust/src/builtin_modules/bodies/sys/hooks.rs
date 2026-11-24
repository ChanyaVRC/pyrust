// Profiling, tracing, asyncgen, auditing, display, exception, unraisable, and
// breakpoint hooks for the `sys` module.

use std::rc::Rc;

use super::platform_info::make_asyncgen_hooks;
use super::runtime_limits::{require_exact_one_arg, require_no_args, require_single_named_arg};
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, cached_builtins_module, reject_keyword_args_expanded,
    render_value_repr, value_type_name_str,
};
use crate::value::{Value, ValueKind};

/// Check whether `value` is callable, matching `builtins.callable()`.
fn sys_value_is_callable(value: &Value) -> bool {
    match value.kind() {
        ValueKind::UserFunction(_)
        | ValueKind::BuiltinFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. }
        | ValueKind::PyClass(_) => true,
        ValueKind::BuiltinObject { .. } => crate::interpreter::is_builtin_callable_adapter(value),
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            crate::interpreter::lookup_class_attr(&class, "__call__").is_some()
        }
        _ => false,
    }
}

fn write_to_sys_stream(
    interp: &mut Interpreter,
    stream_name: &str,
    line: String,
    require_stream: bool,
) -> Result<()> {
    let sys_mod = interp.load_module("sys")?;
    let stream = interp.get_attr(&sys_mod, stream_name)?;
    if stream.is_none() {
        if require_stream {
            return Err(PyError::Runtime(format!("lost sys.{stream_name}")));
        }
        return Ok(());
    }
    let write_fn = interp.get_attr(&stream, "write")?;
    interp.call_function_expanded(
        write_fn,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(line),
        }],
    )?;
    Ok(())
}

pub(super) fn getprofile(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "getprofile")?;
    Ok(interp.sys_state.borrow().profile_func.clone())
}

pub(super) fn setprofile(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let func = require_exact_one_arg(args, fn_name, "setprofile")?;
    interp.sys_state.borrow_mut().profile_func = func.clone();
    Ok(Value::none())
}

pub(super) fn gettrace(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "gettrace")?;
    Ok(interp.sys_state.borrow().trace_func.clone())
}

pub(super) fn settrace(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let func = require_exact_one_arg(args, fn_name, "settrace")?;
    interp.sys_state.borrow_mut().trace_func = func.clone();
    Ok(Value::none())
}

pub(super) fn get_asyncgen_hooks(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "get_asyncgen_hooks")?;
    let (firstiter, finalizer) = {
        let state = interp.sys_state.borrow();
        (
            state.asyncgen_firstiter.clone(),
            state.asyncgen_finalizer.clone(),
        )
    };
    Ok(make_asyncgen_hooks(firstiter, finalizer))
}

pub(super) fn set_asyncgen_hooks(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    if args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "set_asyncgen_hooks() takes at most 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let mut firstiter: Option<Value> = None;
    let mut finalizer: Option<Value> = None;
    let mut pos_idx = 0usize;
    for arg in args {
        match arg.name.as_deref() {
            None => {
                if pos_idx == 0 {
                    firstiter = Some(arg.value.clone());
                } else {
                    finalizer = Some(arg.value.clone());
                }
                pos_idx += 1;
            }
            Some("firstiter") => {
                if firstiter.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "argument for set_asyncgen_hooks() given by name ('firstiter') and position (1)".to_string(),
                    ));
                }
                firstiter = Some(arg.value.clone());
            }
            Some("finalizer") => {
                if finalizer.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        "argument for set_asyncgen_hooks() given by name ('finalizer') and position (2)".to_string(),
                    ));
                }
                finalizer = Some(arg.value.clone());
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{other}' is an invalid keyword argument for set_asyncgen_hooks()"),
                ));
            }
        }
    }
    for (slot_name, hook) in [("firstiter", &firstiter), ("finalizer", &finalizer)] {
        if let Some(f) = hook
            && !f.is_none()
            && !sys_value_is_callable(f)
        {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "callable {slot_name} expected, got {}",
                    value_type_name_str(f)
                ),
            ));
        }
    }
    let mut state = interp.sys_state.borrow_mut();
    if let Some(f) = firstiter {
        state.asyncgen_firstiter = f;
    }
    if let Some(f) = finalizer {
        state.asyncgen_finalizer = f;
    }
    Ok(Value::none())
}

pub(super) fn addaudithook(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let hook = require_single_named_arg(args, "addaudithook", "hook")?.clone();
    if !sys_value_is_callable(&hook) {
        return Err(PyError::named(
            "TypeError",
            format!(
                "addaudithook() argument must be callable, not {}",
                value_type_name_str(&hook)
            ),
        ));
    }
    interp.sys_state.borrow_mut().audit_hooks.push(hook);
    Ok(Value::none())
}

pub(super) fn audit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "audit() missing required argument 'event' (pos 1)".to_string(),
        ));
    }
    let event = args[0].value.clone();
    if event.as_str().is_none() {
        return Err(PyError::named(
            "TypeError",
            format!(
                "audit() argument 1 must be str, not {}",
                value_type_name_str(&event)
            ),
        ));
    }
    let hooks: Vec<Value> = interp.sys_state.borrow().audit_hooks.clone();
    if hooks.is_empty() {
        return Ok(Value::none());
    }
    let event_args = Value::tuple(args[1..].iter().map(|a| a.value.clone()).collect());
    let call_args = [
        ExpandedCallArg {
            name: None,
            value: event,
        },
        ExpandedCallArg {
            name: None,
            value: event_args,
        },
    ];
    for hook in hooks {
        interp.call_function_expanded(hook, &call_args)?;
    }
    Ok(Value::none())
}

pub(super) fn displayhook(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let val = require_exact_one_arg(args, fn_name, "displayhook")?.clone();
    if val.is_none() {
        return Ok(Value::none());
    }
    let rendered = render_value_repr(interp, &val)?;
    write_to_sys_stream(interp, "stdout", format!("{rendered}\n"), true)?;
    let builtins_mod = cached_builtins_module();
    if let ValueKind::PyModule(m) = builtins_mod.kind() {
        m.borrow_mut().insert_attr("_".to_string(), val);
    }
    Ok(Value::none())
}

pub(super) fn excepthook(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "excepthook() takes exactly 3 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let exctype = &args[0].value;
    let value = &args[1].value;
    let type_name = match exctype.kind() {
        ValueKind::PyClass(c) => c.borrow().name.clone(),
        _ => value_type_name_str(value).to_string(),
    };
    let msg = value.to_py_str();
    let line = if msg.is_empty() {
        format!("{type_name}\n")
    } else {
        format!("{type_name}: {msg}\n")
    };
    write_to_sys_stream(interp, "stderr", line, false)?;
    Ok(Value::none())
}

pub(super) fn unraisablehook(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let unraisable = require_single_named_arg(args, "unraisablehook", "unraisable")?;
    let sys_mod = interp.load_module("sys")?;
    let stderr = interp.get_attr(&sys_mod, "stderr")?;
    if !stderr.is_none() {
        let msg = interp
            .get_attr(unraisable, "err_msg")
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "Exception ignored in".to_string());
        let write_fn = interp.get_attr(&stderr, "write")?;
        interp.call_function_expanded(
            write_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::string(format!("{msg}\n")),
            }],
        )?;
    }
    Ok(Value::none())
}

pub(super) fn breakpointhook(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let hookname = match std::env::var("PYTHONBREAKPOINT") {
        Ok(v) if v == "0" => return Ok(Value::none()),
        Ok(v) if !v.is_empty() => v,
        _ => return Ok(Value::none()),
    };
    let (mod_name, attr_path) = match hookname.rsplit_once('.') {
        Some((m, a)) if !m.is_empty() && !a.is_empty() => (m, a),
        Some(_) => return Ok(Value::none()),
        None => ("builtins", hookname.as_str()),
    };
    let Ok(module) = interp.load_module(mod_name) else {
        return Ok(Value::none());
    };
    let Ok(hook) = interp.get_attr(&module, attr_path) else {
        return Ok(Value::none());
    };
    interp.call_function_expanded(hook, args)
}

pub(super) fn call_tracing(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "call_tracing() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let func = args[0].value.clone();
    let Some(items) = args[1].value.as_tuple() else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "call_tracing() argument 2 must be tuple, not {}",
                value_type_name_str(&args[1].value)
            ),
        ));
    };
    let call_args: Vec<ExpandedCallArg> = items
        .iter()
        .map(|v| ExpandedCallArg {
            name: None,
            value: v.clone(),
        })
        .collect();
    interp.call_function_expanded(func, &call_args)
}
