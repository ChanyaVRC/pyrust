//! `code` object — the value returned by `function.__code__`.
//!
//! pyrust does not model a full CPython code object (no bytecode disassembly,
//! line tables, etc.).  This is a lightweight `BuiltinObject` that carries the
//! introspection attributes `inspect` / `functools` most commonly read:
//! `co_name`, `co_varnames`, `co_argcount`.  It exists so that
//! `f.__code__` does not `AttributeError` and the common code-object reads
//! work (issue #1959).

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value};

/// Backing state for a `code` object.
pub struct CodeState {
    /// `co_name` — the function's declared name.
    pub name: String,
    /// `co_argcount` — number of positional parameters (excludes `*args`,
    /// `**kwargs`, and keyword-only parameters), matching CPython.
    pub argcount: i64,
    /// `co_varnames` — a tuple of local variable name strings.  pyrust supplies
    /// the parameter names in CPython order (positional, keyword-only, `*args`,
    /// `**kwargs`); other locals are not enumerated.
    pub varnames: Value,
}

pub struct CodeOps;
pub const CODE_OPS: &CodeOps = &CodeOps;
pub const TYPE_NAME: &str = "code";

impl BuiltinTypeOps for CodeOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        match borrow.downcast_ref::<CodeState>() {
            Some(s) => format!("<code object {} at 0x0>", s.name),
            None => "<code object>".to_string(),
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<CodeState>()?;
        match name {
            "co_name" => Some(Value::string(s.name.clone())),
            "co_argcount" => Some(Value::int(s.argcount)),
            "co_varnames" => Some(s.varnames.clone()),
            _ => None,
        }
    }
}

/// Construct a `code` object value carrying the given introspection fields.
pub fn code(name: String, argcount: i64, varnames: Vec<Value>) -> Value {
    let state: Box<dyn Any> = Box::new(CodeState {
        name,
        argcount,
        varnames: Value::tuple(varnames),
    });
    Value::builtin_object(CODE_OPS, state)
}
