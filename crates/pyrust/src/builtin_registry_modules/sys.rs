//! `sys` module built-ins — declared via the file-scoped `pyrust_module!`
//! macro.  Reference: <https://docs.python.org/3/library/sys.html>

use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{
    instantiate_exception, lookup_name_in_module, reject_keyword_args_expanded,
};
use crate::value::{Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    name = "sys",

    constants {
        "version" => Value::string("PyRust 0.2"),
        "argv"    => Value::list(Vec::new()),
    }

    /// CPython: sys.exit([arg]) — raises `SystemExit(arg)`.
    /// <https://docs.python.org/3/library/sys.html#sys.exit>
    fn exit(args) -> Result<Value> {
        reject_keyword_args_expanded("sys.exit", args)?;
        let arg = if args.is_empty() { Value::int(0) } else { args[0].value.clone() };
        let class = match lookup_name_in_module(&_interp.env, "SystemExit") {
            Some(v) => match v.kind() {
                ValueKind::PyClass(c) => Rc::clone(c),
                _ => return Err(PyError::Runtime(
                    "built-in exception 'SystemExit' is not defined".to_string(),
                )),
            },
            None => return Err(PyError::Runtime(
                "built-in exception 'SystemExit' is not defined".to_string(),
            )),
        };
        let exc = instantiate_exception(class, vec![arg]);
        Err(PyError::Raised(exc))
    }
}
