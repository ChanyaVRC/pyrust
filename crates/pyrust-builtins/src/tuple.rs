use pyrust_core::{PyError, Result, Value};

use crate::method_signature::{KeywordPolicy, PositionalArity};
use crate::sequence;

pub const TYPE_NAME: &str = "tuple";

/// Canonical list of method names dispatched by `call`.
pub const METHODS: &[&str] = &["__iter__", "index", "count", "__getnewargs__"];

pub const CLASS_ATTRS: crate::primitive_class_attrs::PrimitiveClassAttrs =
    crate::primitive_class_attrs::PrimitiveClassAttrs::new(TYPE_NAME, METHODS).with_flags(
        crate::primitive_class_attrs::PrimitiveClassFlags::NONE
            .with_init()
            .with_new()
            .with_class_getitem(),
    );

/// Returns `true` if `method` is the name of a built-in `tuple` method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Positional signature for every public tuple method.
pub fn positional_arity(method: &str) -> Option<PositionalArity> {
    Some(match method {
        "__iter__" | "__getnewargs__" => PositionalArity::exact(0),
        "count" => PositionalArity::exact(1),
        "index" => PositionalArity::range(1, 3),
        _ => return None,
    })
}

#[inline]
pub fn validate_method_positional_arity(method: &str, given: usize) -> Result<()> {
    if given == 0 {
        return Ok(());
    }
    match positional_arity(method) {
        Some(arity) => arity.reject_excess(TYPE_NAME, method, given),
        None => Ok(()),
    }
}

#[inline]
pub fn validate_method_keywords(method: &str, has_keywords: bool) -> Result<()> {
    if !has_keywords {
        return Ok(());
    }
    match positional_arity(method) {
        Some(_) => KeywordPolicy::Reject.validate(TYPE_NAME, method, true),
        None => Ok(()),
    }
}

pub fn keyword_policy(method: &str) -> Option<KeywordPolicy> {
    positional_arity(method).map(|_| KeywordPolicy::Reject)
}

/// Interpreter-owned route for tuple methods that can invoke `__eq__`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterpreterMethod {
    Index,
    Count,
}

/// Classify a tuple method that needs mutable interpreter access.
pub fn interpreter_method(method: &str) -> Option<InterpreterMethod> {
    match method {
        "index" => Some(InterpreterMethod::Index),
        "count" => Some(InterpreterMethod::Count),
        _ => None,
    }
}

/// Compatibility predicate for callers that have not migrated to the typed
/// [`InterpreterMethod`] route yet.
#[deprecated(since = "0.1.0", note = "use interpreter_method(method).is_some()")]
pub fn requires_interpreter(method: &str) -> bool {
    interpreter_method(method).is_some()
}

pub fn call(method: &str, items: &[Value], args: Vec<Value>) -> Result<Value> {
    validate_method_positional_arity(method, args.len())?;
    call_prevalidated(method, items, args)
}

/// Dispatch after the interpreter adapter has already validated arity.
#[doc(hidden)]
pub fn call_prevalidated(method: &str, items: &[Value], args: Vec<Value>) -> Result<Value> {
    match method {
        "index" => sequence::seq_index(items, &args, "tuple"),
        "count" => sequence::seq_count(items, &args, "tuple"),
        // __getnewargs__ supports the pickle protocol: it returns a 1-tuple
        // containing the tuple itself, i.e. (1, 2).__getnewargs__() == ((1, 2),).
        "__getnewargs__" => {
            if !args.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "tuple.__getnewargs__() takes no arguments ({} given)",
                        args.len()
                    ),
                ));
            }
            Ok(Value::tuple(vec![Value::tuple(items.to_vec())]))
        }
        // Intercepted by the interpreter's iteration domain; drift sentinel.
        "__iter__" => Err(PyError::named(
            "TypeError",
            "'tuple' __iter__ must be dispatched by the interpreter",
        )),
        _ => Err(PyError::named(
            "AttributeError",
            format!("'tuple' object has no attribute '{method}'"),
        )),
    }
}
