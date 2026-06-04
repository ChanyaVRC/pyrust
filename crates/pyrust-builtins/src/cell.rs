//! `cell` object — an element of `function.__closure__` (issue #2106).
//!
//! pyrust resolves a closure's free variables through the captured `env` chain
//! rather than CPython-style cell objects.  `function.__closure__` still has to
//! report a tuple of `cell` objects (one per free variable, in `co_freevars`
//! order), each exposing a readable `cell_contents`.  This is a lightweight
//! `BuiltinObject` carrying the captured value; pyrust does not model cell
//! mutation through this object (closures read the live env entry directly), so
//! `cell_contents` is a snapshot taken when `__closure__` is accessed — which
//! is sufficient for the introspection consumers (`inspect`, REPL) that read it.

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value};

pub struct CellState {
    /// `cell_contents` — the captured free-variable value.
    pub contents: Value,
}

pub struct CellOps;
pub const CELL_OPS: &CellOps = &CellOps;
pub const TYPE_NAME: &str = "cell";

impl BuiltinTypeOps for CellOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        // CPython: `<cell at 0x...: int object at 0x...>`.  The addresses are
        // implementation-specific; report 0x0 (matching the convention used by
        // pyrust's other lightweight introspection objects, e.g. `code`).  The
        // contents' type name is reported so the structure matches CPython.
        let borrow = state.borrow();
        match borrow.downcast_ref::<CellState>() {
            Some(s) => {
                let type_name = pyrust_core::builtin_type_name(&s.contents);
                format!("<cell at 0x0: {type_name} object at 0x0>")
            }
            None => "<cell at 0x0: empty>".to_string(),
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<CellState>()?;
        match name {
            "cell_contents" => Some(s.contents.clone()),
            _ => None,
        }
    }
}

/// Construct a `cell` object wrapping `contents`.
pub fn cell(contents: Value) -> Value {
    let state: Box<dyn Any> = Box::new(CellState { contents });
    Value::builtin_object(CELL_OPS, state)
}
