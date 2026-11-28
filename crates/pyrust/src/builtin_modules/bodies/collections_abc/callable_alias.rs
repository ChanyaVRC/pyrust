//! `collections.abc.Callable[[A, B], R]`: CPython's `_CallableGenericAlias`,
//! plus the `_is_param_expr` / `_type_repr` helpers `_collections_abc`
//! exposes.

use crate::error::{PyError, Result};
use crate::interpreter::{
    Interpreter, function_type_singleton, render_value_repr, type_class_singleton, value_class,
};
use crate::value::{Value, ValueKind};

use super::support::{call, isinstance};

pub(super) const CALLABLE_ALIAS_DOC: &str = " Represent `Callable[argtypes, resulttype]`.\n\n    \
This sets ``__args__`` to a tuple containing the flattened ``argtypes``\n    \
followed by ``resulttype``.\n\n    \
Example: ``Callable[[int, str], float]`` sets ``__args__`` to\n    \
``(int, str, float)``.\n    ";

/// `_CallableGenericAlias(origin, args)` (also `Callable.__class_getitem__`).
pub(super) fn new_alias(interp: &mut Interpreter, origin: &Value, args: &Value) -> Result<Value> {
    let ValueKind::Tuple(items) = args.kind() else {
        return Err(callable_usage_error());
    };
    let [t_args, t_result] = items else {
        return Err(callable_usage_error());
    };
    let params: Option<Vec<Value>> = match t_args.kind() {
        ValueKind::Tuple(params) => Some(params.to_vec()),
        ValueKind::List(params) => Some(params.to_vec()),
        _ => None,
    };
    let flattened = match params {
        Some(mut params) => {
            params.push(t_result.clone());
            Value::tuple(params)
        }
        _ if is_param_expr(interp, t_args)? => args.clone(),
        _ => {
            let shown = call(
                interp,
                Value::builtin_function("str"),
                std::slice::from_ref(t_args),
            )?;
            return Err(PyError::named(
                "TypeError",
                format!(
                    "Expected a list of types, an ellipsis, ParamSpec, or Concatenate. Got {}",
                    shown.as_str().unwrap_or_default()
                ),
            ));
        }
    };
    Ok(pyrust_builtins::generic_alias::callable_generic_alias(
        origin.clone(),
        flattened,
    ))
}

fn callable_usage_error() -> PyError {
    PyError::named(
        "TypeError",
        "Callable must be used as Callable[[arg, ...], result].",
    )
}

/// `_is_param_expr(obj)`: `...`, a list, or a `typing.ParamSpec` /
/// `typing._ConcatenateGenericAlias` instance.
pub(super) fn is_param_expr(interp: &mut Interpreter, obj: &Value) -> Result<bool> {
    if matches!(obj.kind(), ValueKind::Ellipsis | ValueKind::List(_)) {
        return Ok(true);
    }
    let class = value_class(obj);
    let module = interp.get_attr(&class, "__module__")?;
    if module.as_str() != Some("typing") {
        return Ok(false);
    }
    let name = interp.get_attr(&class, "__name__")?;
    Ok(matches!(
        name.as_str(),
        Some("ParamSpec" | "_ConcatenateGenericAlias")
    ))
}

/// `_type_repr(obj)`: the `repr()` of `obj`, special-casing types.
pub(super) fn type_repr(interp: &mut Interpreter, obj: &Value) -> Result<Value> {
    if isinstance(interp, obj, Value::py_class(type_class_singleton()))? {
        let module = interp.get_attr(obj, "__module__")?;
        let qualname = interp.get_attr(obj, "__qualname__")?;
        let qualname = text_of(interp, &qualname)?;
        if module.as_str() == Some("builtins") {
            return Ok(Value::string(qualname));
        }
        let module = text_of(interp, &module)?;
        return Ok(Value::string(format!("{module}.{qualname}")));
    }
    if matches!(obj.kind(), ValueKind::Ellipsis) {
        return Ok(Value::string("..."));
    }
    if isinstance(interp, obj, Value::py_class(function_type_singleton()))? {
        return interp.get_attr(obj, "__name__");
    }
    Ok(Value::string(render_value_repr(interp, obj)?))
}

/// `f'{value}'` for a module / qualname attribute.
fn text_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_string());
    }
    let text = call(
        interp,
        Value::builtin_function("str"),
        std::slice::from_ref(value),
    )?;
    Ok(text.as_str().unwrap_or_default().to_string())
}
