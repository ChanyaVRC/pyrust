// `socketserver` module — body for the `socketserver @inject` entry in
// `pyrust_builtin_modules!` (issue #3223).
//
// Pure-Python generic socket server framework (`BaseServer`, `TCPServer`,
// `UDPServer`, `ThreadingMixIn`, `ThreadingTCPServer`, `ThreadingUDPServer`,
// `BaseRequestHandler`, `StreamRequestHandler`, `DatagramRequestHandler`)
// built on `socket`, `selectors`, `threading`, and `io`, aligned with
// CPython 3.12's `Lib/socketserver.py`.
//
// Reference: <https://docs.python.org/3/library/socketserver.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `socketserver` module.
const SOCKETSERVER_PY_SOURCE: &str = include_str!("socketserver_py.py");

/// Public names exported from `SOCKETSERVER_PY_SOURCE`.
const SOCKETSERVER_PY_EXPORTS: &[&str] = &[
    "__all__",
    "__version__",
    "BaseServer",
    "TCPServer",
    "UDPServer",
    "ThreadingMixIn",
    "ThreadingUDPServer",
    "ThreadingTCPServer",
    "BaseRequestHandler",
    "StreamRequestHandler",
    "DatagramRequestHandler",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(SOCKETSERVER_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("socketserver: exec namespace not a dict".into()))?;
    for &name in SOCKETSERVER_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
