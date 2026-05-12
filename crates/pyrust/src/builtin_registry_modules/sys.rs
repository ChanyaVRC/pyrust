//! `sys` module built-ins, migrated to the `#[pyfunction]` registry pattern.
//!
//! See <https://docs.python.org/3/library/sys.html> for the canonical
//! signatures.

use std::rc::Rc;

use crate::Interpreter;
use crate::builtin_registry::BuiltinReg;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{
    instantiate_exception, lookup_name_in_module, reject_keyword_args_expanded,
};
use crate::value::{Value, ValueKind};
use pyrust_derive::pyfunction;

/// CPython: `sys.exit([arg])` — raises `SystemExit(arg)`.
/// <https://docs.python.org/3/library/sys.html#sys.exit>
#[pyfunction(name = "sys.exit")]
fn sys_exit(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded("sys.exit", args)?;
    let arg = if args.is_empty() {
        Value::int(0)
    } else {
        args[0].value.clone()
    };
    // Raise SystemExit like CPython — lets finally/with handlers run.
    // Look up the SystemExit class and instantiate it with the original arg
    // so program.rs can extract the integer exit code without reparsing a string.
    let class = match lookup_name_in_module(&interp.env, "SystemExit") {
        Some(v) => match v.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::Runtime(
                    "built-in exception 'SystemExit' is not defined".to_string(),
                ));
            }
        },
        None => {
            return Err(PyError::Runtime(
                "built-in exception 'SystemExit' is not defined".to_string(),
            ));
        }
    };
    let exc = instantiate_exception(class, vec![arg]);
    Err(PyError::Raised(exc))
}

/// Slice of every `sys.*` registration in this file.
pub(crate) const REGS: &[BuiltinReg] = &[SYS_EXIT];
