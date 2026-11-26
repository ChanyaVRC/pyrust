// Interpreter introspection, exit, exception state, frame snapshots, encoding
// queries, and memory helpers for the `sys` module.

use std::rc::Rc;

use super::runtime_limits::{require_exact_one_arg, require_no_args};
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, instantiate_exception, lookup_name_in_module,
    reject_keyword_args_expanded, value_type_name_str,
};
use crate::value::{Value, ValueKind};

pub(super) fn exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    let arg = if args.is_empty() {
        Value::int(0)
    } else {
        args[0].value.clone()
    };
    let class = match lookup_name_in_module(&interp.env, "SystemExit") {
        Some(v) => match v.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::runtime(
                    "built-in exception 'SystemExit' is not defined".to_string(),
                ));
            }
        },
        None => {
            return Err(PyError::runtime(
                "built-in exception 'SystemExit' is not defined".to_string(),
            ));
        }
    };
    let exc = instantiate_exception(class, vec![arg]);
    Err(PyError::raised(exc))
}

pub(super) fn exc_info(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "sys.exc_info")?;
    match interp.active_exception.clone() {
        None => Ok(Value::tuple(vec![
            Value::none(),
            Value::none(),
            Value::none(),
        ])),
        Some(exc_val) => {
            let exc_type = match exc_val.kind() {
                ValueKind::PyInstance(inst) => Value::py_class(Rc::clone(&inst.borrow().class)),
                _ => Value::none(),
            };
            // Issue #2170: the third element is the exception's traceback
            // object (its `__traceback__`), not `None`.
            let tb = match exc_val.kind() {
                ValueKind::PyInstance(inst) => {
                    // The carried traceback lives in C-style slot storage,
                    // independent of any replacement `__dict__`.
                    let raw = inst
                        .borrow()
                        .attrs
                        .get_cloned_or_slot("__traceback__")
                        .unwrap_or_else(Value::none);
                    // Issue #2351: materialise a deferred placeholder and
                    // write the real chain back so repeat reads share it.
                    if let Some(real) = interp.materialize_deferred_traceback(&raw) {
                        inst.borrow_mut()
                            .attrs
                            .insert_slot("__traceback__", real.clone());
                        real
                    } else {
                        raw
                    }
                }
                _ => Value::none(),
            };
            Ok(Value::tuple(vec![exc_type, exc_val, tb]))
        }
    }
}

pub(super) fn getframe(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "_getframe() takes at most 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let depth = if args.is_empty() {
        0usize
    } else {
        let n =
            interp.value_to_isize(&args[0].value, "Python int too large to convert to C int")?;
        if n < 0 {
            // CPython treats a negative depth as the innermost frame.
            0usize
        } else {
            n as usize
        }
    };
    // The innermost `vm_frame_views` entry is the frame that called
    // `_getframe` (builtins push no view of their own).
    let frame = interp.build_frame_object(depth, pyrust_core::get_current_vm_line() as i64);
    if frame.is_none() {
        return Err(PyError::named(
            "ValueError",
            "call stack is not deep enough".to_string(),
        ));
    }
    Ok(frame)
}

pub(super) fn exception(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "sys.exception")?;
    Ok(interp.active_exception.clone().unwrap_or_else(Value::none))
}

pub(super) fn intern(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let val = require_exact_one_arg(args, fn_name, "intern")?;
    match val.kind() {
        ValueKind::Str(_) => Ok(val.clone()),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "intern() argument 1 must be str, not {}",
                value_type_name_str(val),
            ),
        )),
    }
}

pub(super) fn getsizeof(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.is_empty() || args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("getsizeof() takes 1 or 2 arguments ({} given)", args.len()),
        ));
    }
    Ok(Value::int(approximate_sizeof(&args[0].value)))
}

pub(super) fn getdefaultencoding(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_no_args(args, fn_name, "getdefaultencoding")?;
    Ok(Value::string("utf-8"))
}

pub(super) fn is_finalizing(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_no_args(args, fn_name, "is_finalizing")?;
    Ok(Value::bool_(false))
}

pub(super) fn getfilesystemencoding(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_no_args(args, fn_name, "getfilesystemencoding")?;
    Ok(Value::string("utf-8"))
}

pub(super) fn getfilesystemencodeerrors(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_no_args(args, fn_name, "getfilesystemencodeerrors")?;
    let err = if cfg!(windows) {
        "surrogatepass"
    } else {
        "surrogateescape"
    };
    Ok(Value::string(err))
}

pub(super) fn getallocatedblocks(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_no_args(args, fn_name, "getallocatedblocks")?;
    Ok(Value::int(1))
}

pub(super) fn clear_type_cache(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_no_args(args, fn_name, "_clear_type_cache")?;
    Ok(Value::none())
}

/// Return a stable positive integer thread identifier matching
/// `threading.get_ident()`.
fn current_thread_ident() -> i64 {
    crate::interpreter::current_os_thread_ident()
}

pub(super) fn getrefcount(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let val = require_exact_one_arg(args, fn_name, "getrefcount")?;
    Ok(Value::int(val.refcount() as i64))
}

pub(super) fn getframemodulename(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "_getframemodulename() takes at most 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let depth_arg = if let Some(first) = args.first() {
        if let Some(ref kw) = first.name
            && kw != "depth"
        {
            return Err(PyError::named(
                "TypeError",
                format!("'{kw}' is an invalid keyword argument for _getframemodulename()"),
            ));
        }
        let n = interp.value_to_isize(&first.value, "Python int too large to convert to C int")?;
        if n < 0 { 0usize } else { n as usize }
    } else {
        0usize
    };
    let frame = interp.build_frame_object(depth_arg, pyrust_core::get_current_vm_line() as i64);
    if frame.is_none() {
        return Ok(Value::none());
    }
    let globals = interp.get_attr(&frame, "f_globals")?;
    if let Some(dict) = globals.as_dict()
        && let Some(name_val) = dict.get(&crate::value::PyKey::str_from("__name__"))
        && name_val.is_str()
    {
        return Ok(name_val.clone());
    }
    let _ = fn_name;
    Ok(Value::none())
}

pub(super) fn current_frames(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "_current_frames")?;
    let mut d = crate::value::PyDict::default();
    let frame = interp.build_frame_object(0, pyrust_core::get_current_vm_line() as i64);
    if !frame.is_none() {
        d.insert(crate::value::PyKey::Int(current_thread_ident()), frame);
    }
    Ok(Value::dict(d))
}

pub(super) fn current_exceptions(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    require_no_args(args, fn_name, "_current_exceptions")?;
    let mut d = crate::value::PyDict::default();
    let exc = interp.active_exception.clone().unwrap_or_else(Value::none);
    d.insert(crate::value::PyKey::Int(current_thread_ident()), exc);
    Ok(Value::dict(d))
}

/// Rough in-memory size estimate for `sys.getsizeof`.  CPython sizes are
/// implementation-specific; this returns a plausible positive number so
/// callers don't hit AttributeError, but the value must not be compared
/// against CPython in parity tests.
fn approximate_sizeof(value: &Value) -> i64 {
    match value.kind() {
        ValueKind::Str(s) => 49 + s.len() as i64,
        ValueKind::Bytes(b) => 33 + b.len() as i64,
        ValueKind::List(items) => 56 + (items.len() as i64) * 8,
        ValueKind::Tuple(items) => 40 + (items.len() as i64) * 8,
        ValueKind::Int(_) | ValueKind::Bool(_) => 28,
        ValueKind::Float(_) => 24,
        ValueKind::None => 16,
        _ => 16,
    }
}
