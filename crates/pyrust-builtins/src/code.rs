//! `code` object — the value returned by `function.__code__`.
//!
//! pyrust does not model a full CPython code object (no bytecode disassembly,
//! line tables, etc.).  This is a lightweight `BuiltinObject` that carries the
//! introspection attributes `inspect` / `functools` / `traceback` most commonly
//! read: `co_name`, `co_varnames`, `co_argcount`, plus the best-effort
//! `co_flags`, `co_filename`, `co_firstlineno`, `co_consts`, `co_names`
//! (issues #1959 and #2171).

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value};

// CPython code-object flag bits (subset that pyrust can determine).
pub const CO_OPTIMIZED: i64 = 0x0001;
pub const CO_NEWLOCALS: i64 = 0x0002;
pub const CO_VARARGS: i64 = 0x0004;
pub const CO_VARKEYWORDS: i64 = 0x0008;
pub const CO_GENERATOR: i64 = 0x0020;

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
    /// `co_flags` — the standard CPython flag bitmask (best-effort: the
    /// `CO_OPTIMIZED`/`CO_NEWLOCALS`/`CO_VARARGS`/`CO_VARKEYWORDS`/`CO_GENERATOR`
    /// bits pyrust can determine from the function signature/body).
    pub flags: i64,
    /// `co_filename` — path to the source file the code was compiled from,
    /// or `"<unknown>"` when not available (e.g. REPL).
    pub filename: String,
    /// `co_firstlineno` — 1-based source line of the `def`/`<module>`,
    /// best-effort (0 when no line information is available).
    pub firstlineno: i64,
    /// `co_consts` — a tuple of the literals in the constant pool.
    pub consts: Value,
    /// `co_names` — a tuple of the global/attribute name strings.
    pub names: Value,
    /// `co_freevars` — a tuple of the free-variable name strings (names bound
    /// in an enclosing function scope), in CPython's sorted order.  Empty tuple
    /// when the function is not a closure (issue #2106).
    pub freevars: Value,
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
            Some(s) => format!(
                "<code object {} at 0x0, file \"{}\", line {}>",
                s.name, s.filename, s.firstlineno
            ),
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
            "co_flags" => Some(Value::int(s.flags)),
            "co_filename" => Some(Value::string(s.filename.clone())),
            "co_firstlineno" => Some(Value::int(s.firstlineno)),
            "co_consts" => Some(s.consts.clone()),
            "co_names" => Some(s.names.clone()),
            "co_freevars" => Some(s.freevars.clone()),
            _ => None,
        }
    }
}

/// Construct a `code` object value carrying the full set of introspection
/// fields pyrust can supply.
#[allow(clippy::too_many_arguments)]
pub fn code_full(
    name: String,
    argcount: i64,
    varnames: Vec<Value>,
    flags: i64,
    filename: String,
    firstlineno: i64,
    consts: Vec<Value>,
    names: Vec<Value>,
    freevars: Vec<Value>,
) -> Value {
    let state: Box<dyn Any> = Box::new(CodeState {
        name,
        argcount,
        varnames: Value::tuple(varnames),
        flags,
        filename,
        firstlineno,
        consts: Value::tuple(consts),
        names: Value::tuple(names),
        freevars: Value::tuple(freevars),
    });
    Value::builtin_object(CODE_OPS, state)
}

/// Construct a minimal `code` object value carrying only the name/argcount/
/// varnames introspection fields.  Used where the constant/name pools and
/// line/filename metadata are not readily available (e.g. a traceback frame
/// reconstructed from a `FrameInfo`).
pub fn code(name: String, argcount: i64, varnames: Vec<Value>) -> Value {
    code_full(
        name,
        argcount,
        varnames,
        0,
        "<unknown>".to_string(),
        0,
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
}
