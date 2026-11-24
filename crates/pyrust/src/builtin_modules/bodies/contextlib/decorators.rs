//! `@contextmanager`, `_ContextManagerFactory`, and `_GeneratorContextManager`
//! implementations for `contextlib`.

use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, is_stop_iteration_error};
use crate::value::{InstanceAttrs, Value, ValueKind};

use super::helpers::{expect_self, reject_extra_init_args};
use super::registry::{make_cm_factory, make_generator_manager_instance};

pub(super) fn contextmanager(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes exactly 1 argument"),
        ));
    }
    Ok(make_cm_factory(args[0].value.clone()))
}

pub(super) fn gen_cm_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let _ = expect_self(args, fn_name)?;
    reject_extra_init_args(args, fn_name)
}

pub(super) fn gen_cm_enter(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let generator = inst
        .borrow()
        .attrs
        .get("_gen")
        .cloned()
        .ok_or_else(|| PyError::Runtime(format!("internal: {fn_name}() _gen missing")))?;
    match interp.call_next(&generator, None) {
        Ok(val) => Ok(val),
        Err(e) if is_stop_iteration_error(&e) => Err(PyError::named(
            "RuntimeError",
            "generator didn't yield".to_string(),
        )),
        Err(e) => Err(e),
    }
}

pub(super) fn gen_cm_exit(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let generator = inst
        .borrow()
        .attrs
        .get("_gen")
        .cloned()
        .ok_or_else(|| PyError::Runtime(format!("internal: {fn_name}() _gen missing")))?;
    // args: [self, exc_type, exc_val, traceback]
    let exc_type = args
        .get(1)
        .map(|a| a.value.clone())
        .unwrap_or_else(Value::none);
    let exc_val = args
        .get(2)
        .map(|a| a.value.clone())
        .unwrap_or_else(Value::none);
    if exc_type.is_none() {
        match interp.call_next(&generator, None) {
            Ok(_) => Err(PyError::named(
                "RuntimeError",
                "generator didn't stop".to_string(),
            )),
            Err(e) if is_stop_iteration_error(&e) => Ok(Value::bool_(false)),
            Err(e) => Err(e),
        }
    } else {
        // Materialise a concrete exception instance to throw, mirroring
        // CPython's `if value is None: value = typ()`.
        let to_throw = if !exc_val.is_none() {
            exc_val
        } else if let ValueKind::PyClass(c) = exc_type.kind() {
            crate::interpreter::instantiate_exception(Rc::clone(c), Vec::new())
        } else {
            exc_type
        };
        match interp.call_generator_method(generator, "throw", vec![to_throw.clone()]) {
            Ok(_) => Err(PyError::named(
                "RuntimeError",
                "generator didn't stop after throw()".to_string(),
            )),
            Err(e) if is_stop_iteration_error(&e) => Ok(Value::bool_(true)),
            Err(e) => {
                // Re-raising the exact same exception instance means "not
                // suppressed" (return False); a newly raised exception propagates.
                if is_same_raised_exception(&e, &to_throw) {
                    return Ok(Value::bool_(false));
                }
                Err(e)
            }
        }
    }
}

pub(super) fn factory_init(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_extra_init_args(args, fn_name)
}

pub(super) fn factory_call(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let (factory_class, func) = {
        let factory = inst.borrow();
        let func = factory
            .attrs
            .get("_func")
            .cloned()
            .ok_or_else(|| PyError::Runtime(format!("internal: {fn_name}() _func missing")))?;
        (Rc::clone(&factory.class), func)
    };
    let generator = interp.call_function_expanded(func, &args[1..])?;
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_gen", generator);
    Ok(make_generator_manager_instance(&factory_class, attrs))
}

fn is_same_raised_exception(err: &PyError, thrown: &Value) -> bool {
    let PyError::Raised(raised) = err else {
        return false;
    };
    match (raised.kind(), thrown.kind()) {
        (ValueKind::PyInstance(a), ValueKind::PyInstance(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}
