/// Returns `true` for any `PyError` variant that represents a `StopIteration`
/// or subclass. Used by every generator/coroutine termination path so PEP 380
/// exhaustion and PEP 479 wrapping cannot drift.
///
/// Materialised classes are compared with the canonical built-in exception
/// singleton. Python-visible class names are insufficient: a subclass has a
/// different name, while an unrelated user exception may itself be named
/// `StopIteration`.
#[inline]
pub(crate) fn is_stop_iteration_error(err: &PyError) -> bool {
    match err {
        // VM-internal errors that have not been materialised carry only their
        // canonical built-in name.
        PyError::Named(cls, _) => cls.as_ref() == "StopIteration",
        PyError::Class(cls, _) => class_is_builtin_exception_subclass(cls, "StopIteration"),
        PyError::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => {
                class_is_builtin_exception_subclass(&inst.borrow().class, "StopIteration")
            }
            ValueKind::PyClass(cls) => class_is_builtin_exception_subclass(cls, "StopIteration"),
            _ => false,
        },
        _ => false,
    }
}

/// Extract the `value` attribute from a `StopIteration` error (PEP 380 §3).
///
/// In CPython, `StopIteration.value` is the first positional argument: when
/// a generator does `return x`, the VM raises `StopIteration(x)` and `x` is
/// accessible as `e.value` and `e.args[0]`.
///
/// We mirror this by extracting `args[0]` from the materialized exception
/// instance, or by using the message string for `PyError::Named` variants.
/// Returns `None` when no value was provided (bare `return` / `return None`).
pub(super) fn extract_stop_iteration_value(err: &PyError) -> Option<Value> {
    match err {
        PyError::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => {
                // Check for a `value` attribute first (set by our exception
                // machinery), then fall back to args[0].
                let borrow = inst.borrow();
                if let Some(v) = borrow.attrs.get_slot("value")
                    && !v.is_none()
                {
                    return Some(v.clone());
                }
                // Try args[0].
                if let Some(args_val) = borrow.attrs.get_slot("args") {
                    let first = if let Some(args) = args_val.as_tuple() {
                        args.first().cloned()
                    } else {
                        args_val.as_list().and_then(|args| args.first().cloned())
                    };
                    if let Some(first) = first
                        && !first.is_none()
                    {
                        return Some(first);
                    }
                }
                None
            }
            _ => None,
        },
        PyError::Named(name, msg) if name.as_ref() == "StopIteration" => {
            if msg.is_empty() {
                None
            } else {
                Some(Value::string(msg.clone()))
            }
        }
        _ => None,
    }
}

pub(super) fn pep479_wrap_stop_iteration(err: PyError) -> PyError {
    if !is_stop_iteration_error(&err) {
        return err;
    }

    // Materialise the original StopIteration error into a Value so it can be
    // attached as __cause__ on the new RuntimeError.
    let cause_val: Option<Value> = match err {
        PyError::Raised(exc) => Some(exc),
        PyError::Class(cls, msg) => {
            let args = if msg.is_empty() {
                vec![]
            } else {
                vec![Value::string(msg)]
            };
            Some(instantiate_exception(cls, args))
        }
        PyError::Named(cls_name, msg) => {
            // Internal named errors always denote a canonical built-in class;
            // do not let a module global shadow the exception identity.
            lookup_exc_class(cls_name.as_ref()).map(|cls| {
                let args = if msg.is_empty() {
                    vec![]
                } else {
                    vec![Value::string(msg)]
                };
                instantiate_exception(cls, args)
            })
        }
        _ => None,
    };

    // Build the RuntimeError instance and attach __cause__, __context__, and
    // __suppress_context__, mirroring CPython's PEP 479 behaviour.
    // CPython sets both __cause__ and __context__ to the original StopIteration
    // instance (they are the same object: `e.__context__ is e.__cause__`), and
    // sets __suppress_context__ = True so the "During handling of..." context
    // chain is suppressed in tracebacks.
    if let Some(cause) = cause_val
        && let Some(rt_cls) = lookup_exc_class("RuntimeError")
    {
        let rt_err = instantiate_exception(
            rt_cls,
            vec![Value::string("generator raised StopIteration")],
        );
        if let ValueKind::PyInstance(inst) = rt_err.kind() {
            // Clone before the first insert so both __cause__ and __context__
            // share the same underlying Rc (preserving CPython identity: is).
            let context = cause.clone();
            inst.borrow_mut().attrs.insert_slot("__cause__", cause);
            inst.borrow_mut().attrs.insert_slot("__context__", context);
            inst.borrow_mut()
                .attrs
                .insert_slot("__suppress_context__", Value::bool_(true));
        }
        return PyError::Raised(rt_err);
    }

    // Fallback: builtins not yet installed (startup) or materialisation failed.
    pyrust_core::runtime_err!("generator raised StopIteration")
}
