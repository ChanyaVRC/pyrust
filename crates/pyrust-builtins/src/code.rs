//! `code` object — the value returned by `function.__code__`.
//!
//! pyrust does not model a full CPython code object (no bytecode disassembly,
//! line tables, etc.).  This is a lightweight `BuiltinObject` that carries the
//! introspection attributes `inspect` / `functools` / `traceback` most commonly
//! read: `co_name`, `co_qualname`, `co_varnames`, `co_argcount`, plus the
//! best-effort `co_flags`, `co_filename`, `co_firstlineno`, `co_consts`,
//! `co_names`, `co_freevars`, `co_cellvars`, `co_nlocals`,
//! `co_posonlyargcount`, `co_kwonlyargcount`, `co_stacksize`, `co_code`
//! (issues #1959, #2171, #2185).

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
    /// `co_qualname` — the compile-time qualified name (CPython 3.11+).
    pub qualname: String,
    /// `co_argcount` — number of positional parameters (positional-only plus
    /// positional-or-keyword; excludes `*args`, `**kwargs`, and keyword-only
    /// parameters), matching CPython.
    pub argcount: i64,
    /// `co_posonlyargcount` — number of positional-only parameters.
    pub posonlyargcount: i64,
    /// `co_kwonlyargcount` — number of keyword-only parameters.
    pub kwonlyargcount: i64,
    /// `co_nlocals` — `len(co_varnames)`.
    pub nlocals: i64,
    /// `co_stacksize` — CPython's max operand-stack depth.  pyrust is a register
    /// VM, so this is a best-effort positive int (the register count).
    pub stacksize: i64,
    /// `co_varnames` — a tuple of local variable name strings: the parameters
    /// in CPython order (positional, keyword-only, `*args`, `**kwargs`)
    /// followed by the function-body locals in source order.  Cell variables
    /// are excluded (they appear in `co_cellvars`).
    pub varnames: Value,
    /// `co_flags` — the standard CPython flag bitmask (best-effort: the
    /// `CO_OPTIMIZED`/`CO_NEWLOCALS`/`CO_VARARGS`/`CO_VARKEYWORDS`/`CO_GENERATOR`
    /// bits pyrust can determine from the function signature/body).
    pub flags: i64,
    /// `co_filename` — path to the source file the code was compiled from,
    /// or `"<unknown>"` when not available (e.g. REPL).
    pub filename: String,
    /// `co_firstlineno` — 1-based source line of the `def`/`lambda`/`<module>`,
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
    /// `co_cellvars` — a tuple of the cell-variable name strings (locals
    /// captured by a nested scope), in CPython's sorted order.
    pub cellvars: Value,
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
            "co_qualname" => Some(Value::string(s.qualname.clone())),
            "co_argcount" => Some(Value::int(s.argcount)),
            "co_posonlyargcount" => Some(Value::int(s.posonlyargcount)),
            "co_kwonlyargcount" => Some(Value::int(s.kwonlyargcount)),
            "co_nlocals" => Some(Value::int(s.nlocals)),
            "co_stacksize" => Some(Value::int(s.stacksize)),
            "co_varnames" => Some(s.varnames.clone()),
            "co_flags" => Some(Value::int(s.flags)),
            "co_filename" => Some(Value::string(s.filename.clone())),
            "co_firstlineno" => Some(Value::int(s.firstlineno)),
            "co_consts" => Some(s.consts.clone()),
            "co_names" => Some(s.names.clone()),
            "co_freevars" => Some(s.freevars.clone()),
            "co_cellvars" => Some(s.cellvars.clone()),
            // `co_code` is the raw bytecode bytes.  pyrust's register VM has no
            // CPython-compatible bytecode to expose, so return an empty `bytes`
            // object — the right TYPE for callers that only check `co_code`'s
            // type (e.g. `type(co.co_code) is bytes`).
            "co_code" => Some(Value::bytes(Vec::new())),
            _ => None,
        }
    }
}

/// All fields needed to construct a full `code` object.  Grouped into a struct
/// to keep the introspection call site readable (it supplies ~15 fields).
pub struct CodeBuild {
    pub name: String,
    pub qualname: String,
    pub argcount: i64,
    pub posonlyargcount: i64,
    pub kwonlyargcount: i64,
    pub nlocals: i64,
    pub stacksize: i64,
    pub varnames: Vec<Value>,
    pub flags: i64,
    pub filename: String,
    pub firstlineno: i64,
    pub consts: Vec<Value>,
    pub names: Vec<Value>,
    pub freevars: Vec<Value>,
    pub cellvars: Vec<Value>,
}

impl CodeBuild {
    /// Construct the `code` object value from the collected introspection
    /// fields.
    pub fn build(self) -> Value {
        let state: Box<dyn Any> = Box::new(CodeState {
            name: self.name,
            qualname: self.qualname,
            argcount: self.argcount,
            posonlyargcount: self.posonlyargcount,
            kwonlyargcount: self.kwonlyargcount,
            nlocals: self.nlocals,
            stacksize: self.stacksize,
            varnames: Value::tuple(self.varnames),
            flags: self.flags,
            filename: self.filename,
            firstlineno: self.firstlineno,
            consts: Value::tuple(self.consts),
            names: Value::tuple(self.names),
            freevars: Value::tuple(self.freevars),
            cellvars: Value::tuple(self.cellvars),
        });
        Value::builtin_object(CODE_OPS, state)
    }
}

/// Construct a minimal `code` object value carrying only the name/argcount/
/// varnames introspection fields.  Used where the constant/name pools and
/// line/filename metadata are not readily available (e.g. a `<module>` frame
/// or a traceback frame reconstructed from a `FrameInfo`).  `co_qualname`
/// defaults to `co_name`.
pub fn code(name: String, argcount: i64, varnames: Vec<Value>) -> Value {
    code_with_loc(name, argcount, varnames, "<unknown>".to_string(), 0)
}

/// Like [`code`] but also carries the source `filename` (`co_filename`) and
/// `firstlineno` (`co_firstlineno`).  Used when reconstructing a traceback
/// frame's `f_code` from a captured `FrameInfo`, so that
/// `tb.tb_frame.f_code.co_filename` reports the frame's own source file rather
/// than `<unknown>` (issue #2438).
pub fn code_with_loc(
    name: String,
    argcount: i64,
    varnames: Vec<Value>,
    filename: String,
    firstlineno: i64,
) -> Value {
    let nlocals = varnames.len() as i64;
    CodeBuild {
        qualname: name.clone(),
        name,
        argcount,
        posonlyargcount: 0,
        kwonlyargcount: 0,
        nlocals,
        stacksize: 0,
        varnames,
        flags: 0,
        filename,
        firstlineno,
        consts: Vec::new(),
        names: Vec::new(),
        freevars: Vec::new(),
        cellvars: Vec::new(),
    }
    .build()
}
