//! Small protocol helpers shared by the `contextlib` implementations: calling
//! with `*args` / `**kwds`, attribute access that only treats `AttributeError`
//! as "absent", exception inspection, and the exception shapes CPython raises.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, class_is_subclass_of, key_to_value, lookup_exc_class,
    value_type_name_str,
};
use crate::value::{PyDict, PyInstance, PyKey, Value, ValueKind};

/// The `self` receiver of a native method, which must be a `PyInstance`.
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

pub(super) fn positional(value: Value) -> ExpandedCallArg {
    ExpandedCallArg { name: None, value }
}

pub(super) fn positional_args(values: &[Value]) -> Vec<ExpandedCallArg> {
    values.iter().cloned().map(positional).collect()
}

/// `callable(*values)`.
pub(super) fn call(interp: &mut Interpreter, callable: &Value, values: &[Value]) -> Result<Value> {
    interp.call_function_expanded(callable.clone(), &positional_args(values))
}

/// `obj.name(*values)`.
pub(super) fn call_method(
    interp: &mut Interpreter,
    receiver: &Value,
    name: &str,
    values: &[Value],
) -> Result<Value> {
    let method = interp.get_attr(receiver, name)?;
    call(interp, &method, values)
}

/// `getattr(target, name, None)`: only `AttributeError` means "absent".
pub(super) fn optional_attr(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
) -> Result<Option<Value>> {
    match interp.get_attr(target, name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.class_name_is("AttributeError") => Ok(None),
        Err(error) => Err(error),
    }
}

/// `type(value)`.
pub(super) fn type_of(value: &Value) -> Value {
    crate::interpreter::value_class(value)
}

/// `isinstance(value, <builtin exception class named any of names>)`.
pub(super) fn is_exception_instance(value: &Value, names: &[&str]) -> bool {
    let ValueKind::PyInstance(instance) = value.kind() else {
        return false;
    };
    let class = Rc::clone(&instance.borrow().class);
    names.iter().any(|name| {
        lookup_exc_class(name).is_some_and(|expected| class_is_subclass_of(&class, &expected))
    })
}

/// The exception object carried by `error`, plus an equivalent error that can
/// be re-raised unchanged.
pub(super) fn exception_of(interp: &mut Interpreter, error: PyError) -> Result<(Value, PyError)> {
    let exception = interp.materialize_pyerror(error)?;
    Ok((exception.clone(), PyError::raised(exception)))
}

/// `raise <class_name>(message) from None`.
pub(super) fn error_from_none(
    interp: &mut Interpreter,
    class_name: &'static str,
    message: &str,
) -> PyError {
    let error = PyError::named(class_name, message.to_string());
    let Ok(exception) = interp.materialize_pyerror(error) else {
        return PyError::named(class_name, message.to_string());
    };
    let _ = interp.assign_attr(exception.clone(), "__cause__", Value::none());
    let _ = interp.assign_attr(
        exception.clone(),
        "__suppress_context__",
        Value::bool_(true),
    );
    PyError::raised(exception)
}

/// `TypeError: '{module}.{qualname}' object does not support {protocol}`.
pub(super) fn protocol_error(
    interp: &mut Interpreter,
    object: &Value,
    protocol: &str,
) -> Result<PyError> {
    let class = type_of(object);
    let module = interp.get_attr(&class, "__module__")?;
    let qualname = interp.get_attr(&class, "__qualname__")?;
    let module_text = interp.render_value_as_str(&module)?;
    let qualname_text = interp.render_value_as_str(&qualname)?;
    let cause = PyError::named(
        "TypeError",
        format!("'{module_text}.{qualname_text}' object does not support {protocol}"),
    );
    let exception = interp.materialize_pyerror(cause)?;
    let _ = interp.assign_attr(exception.clone(), "__cause__", Value::none());
    let _ = interp.assign_attr(
        exception.clone(),
        "__suppress_context__",
        Value::bool_(true),
    );
    Ok(PyError::raised(exception))
}

/// Split a call into the `(args, kwds)` pair `*args, **kwds` would capture.
pub(super) fn split_call_args(args: &[ExpandedCallArg]) -> Result<(Value, Value)> {
    let mut positional_values = Vec::new();
    let keywords = Value::dict(PyDict::default());
    for arg in args {
        match &arg.name {
            None => positional_values.push(arg.value.clone()),
            Some(name) => {
                keywords.dict_insert(PyKey::str_from(name), arg.value.clone())?;
            }
        }
    }
    Ok((Value::tuple(positional_values), keywords))
}

/// Expand a stored `(args, kwds)` pair back into call arguments.
pub(super) fn expand_call_args(
    interp: &mut Interpreter,
    args: &Value,
    kwds: &Value,
) -> Result<Vec<ExpandedCallArg>> {
    let mut expanded: Vec<ExpandedCallArg> = interp
        .collect_iterable(args)?
        .into_iter()
        .map(positional)
        .collect();
    let items = match kwds.kind() {
        ValueKind::Dict(dict) => dict
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    for (key, value) in items {
        let name = key_to_value(key);
        let Some(name) = name.as_str().map(str::to_string) else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "keywords must be strings, not {}",
                    value_type_name_str(&name)
                ),
            ));
        };
        expanded.push(ExpandedCallArg {
            name: Some(name),
            value,
        });
    }
    Ok(expanded)
}

/// Read an instance attribute without invoking descriptors.
pub(super) fn instance_attr(instance: &Rc<RefCell<PyInstance>>, name: &str) -> Option<Value> {
    instance.borrow().attrs.get(name).cloned()
}

/// Read an instance attribute that must exist (internal invariant).
pub(super) fn required_attr(
    instance: &Rc<RefCell<PyInstance>>,
    name: &str,
    fn_name: &str,
) -> Result<Value> {
    instance_attr(instance, name)
        .ok_or_else(|| PyError::runtime(format!("internal: {fn_name}() {name} missing")))
}

/// The CPython-style display name of a native method (`closing.__init__`):
/// registry names carry the module prefix.
pub(super) fn display_name(fn_name: &str) -> &str {
    fn_name.strip_prefix("contextlib.").unwrap_or(fn_name)
}

fn quoted_list(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("'{name}'")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [init @ .., last] => format!("{}, and {last}", init.join(", ")),
    }
}

/// Bind the arguments of a native method like CPython binds a Python
/// signature `(self, <required...>, <optional...>)`, producing the same
/// `TypeError` messages. Returns one slot per declared parameter.
pub(super) fn bind_args(
    args: &[ExpandedCallArg],
    fn_name: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<Vec<Option<Value>>> {
    bind_user_args(
        args.get(1..).unwrap_or_default(),
        display_name(fn_name),
        1,
        required,
        optional,
    )
}

/// `bind_args` for a module-level function (no `self`).
pub(super) fn bind_function_args(
    args: &[ExpandedCallArg],
    display: &str,
    required: &[&str],
) -> Result<Vec<Value>> {
    Ok(bind_user_args(args, display, 0, required, &[])?
        .into_iter()
        .flatten()
        .collect())
}

fn bind_user_args(
    user: &[ExpandedCallArg],
    display: &str,
    receivers: usize,
    required: &[&str],
    optional: &[&str],
) -> Result<Vec<Option<Value>>> {
    let params: Vec<&str> = required.iter().chain(optional).copied().collect();
    let positional_count = user.iter().filter(|arg| arg.name.is_none()).count();
    if positional_count > params.len() {
        let expected = if optional.is_empty() {
            format!(
                "{} positional argument{}",
                params.len() + receivers,
                if params.len() + receivers == 1 {
                    ""
                } else {
                    "s"
                }
            )
        } else {
            format!(
                "from {} to {} positional arguments",
                required.len() + receivers,
                params.len() + receivers
            )
        };
        return Err(PyError::named(
            "TypeError",
            format!(
                "{display}() takes {expected} but {} {} given",
                positional_count + receivers,
                if positional_count + receivers == 1 {
                    "was"
                } else {
                    "were"
                }
            ),
        ));
    }
    let mut bound: Vec<Option<Value>> = vec![None; params.len()];
    let mut next_positional = 0;
    for arg in user {
        match arg.name.as_deref() {
            None => {
                bound[next_positional] = Some(arg.value.clone());
                next_positional += 1;
            }
            Some(name) => match params.iter().position(|param| *param == name) {
                Some(index) if bound[index].is_none() => bound[index] = Some(arg.value.clone()),
                Some(_) => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{display}() got multiple values for argument '{name}'"),
                    ));
                }
                None => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{display}() got an unexpected keyword argument '{name}'"),
                    ));
                }
            },
        }
    }
    let missing: Vec<&str> = required
        .iter()
        .enumerate()
        .filter(|(index, _)| bound[*index].is_none())
        .map(|(_, name)| *name)
        .collect();
    if !missing.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{display}() missing {} required positional argument{}: {}",
                missing.len(),
                if missing.len() == 1 { "" } else { "s" },
                quoted_list(&missing)
            ),
        ));
    }
    Ok(bound)
}

/// `bind_args` for signatures with only required parameters.
pub(super) fn bind_required(
    args: &[ExpandedCallArg],
    fn_name: &str,
    required: &[&str],
) -> Result<Vec<Value>> {
    Ok(bind_args(args, fn_name, required, &[])?
        .into_iter()
        .flatten()
        .collect())
}
