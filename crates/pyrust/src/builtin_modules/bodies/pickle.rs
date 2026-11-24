// `pickle` module — body for the `pickle @inject` entry in
// `pyrust_builtin_modules!` (issue #3188).
//
// Pure-Python implementation of CPython 3.12's `pickle` module (`pickle_py.py`),
// wire-compatible with CPython 3.12 across protocols 0 through 5.
//
// Reference: <https://docs.python.org/3/library/pickle.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `pickle` module.
const PICKLE_PY_SOURCE: &str = include_str!("pickle_py.py");

/// Public names exported from `PICKLE_PY_SOURCE` onto the `pickle` module.
const PICKLE_PY_EXPORTS: &[&str] = &[
    "__all__",
    "format_version",
    "compatible_formats",
    "HIGHEST_PROTOCOL",
    "DEFAULT_PROTOCOL",
    "bytes_types",
    "PickleError",
    "PicklingError",
    "UnpicklingError",
    "PickleBuffer",
    "Pickler",
    "Unpickler",
    "_Pickler",
    "_Unpickler",
    "dump",
    "dumps",
    "load",
    "loads",
    "_dump",
    "_dumps",
    "_load",
    "_loads",
    "encode_long",
    "decode_long",
    "whichmodule",
    "ADDITEMS",
    "APPEND",
    "APPENDS",
    "BINBYTES",
    "BINBYTES8",
    "BINFLOAT",
    "BINGET",
    "BININT",
    "BININT1",
    "BININT2",
    "BINPERSID",
    "BINPUT",
    "BINSTRING",
    "BINUNICODE",
    "BINUNICODE8",
    "BUILD",
    "BYTEARRAY8",
    "DICT",
    "DUP",
    "EMPTY_DICT",
    "EMPTY_LIST",
    "EMPTY_SET",
    "EMPTY_TUPLE",
    "EXT1",
    "EXT2",
    "EXT4",
    "FALSE",
    "FLOAT",
    "FRAME",
    "FROZENSET",
    "GET",
    "GLOBAL",
    "INST",
    "INT",
    "LIST",
    "LONG",
    "LONG1",
    "LONG4",
    "LONG_BINGET",
    "LONG_BINPUT",
    "MARK",
    "MEMOIZE",
    "NEWFALSE",
    "NEWOBJ",
    "NEWOBJ_EX",
    "NEWTRUE",
    "NEXT_BUFFER",
    "NONE",
    "OBJ",
    "PERSID",
    "POP",
    "POP_MARK",
    "PROTO",
    "PUT",
    "READONLY_BUFFER",
    "REDUCE",
    "SETITEM",
    "SETITEMS",
    "SHORT_BINBYTES",
    "SHORT_BINSTRING",
    "SHORT_BINUNICODE",
    "STACK_GLOBAL",
    "STOP",
    "STRING",
    "TRUE",
    "TUPLE",
    "TUPLE1",
    "TUPLE2",
    "TUPLE3",
    "UNICODE",
];

/// Execute `PICKLE_PY_SOURCE` once and copy its exported attributes onto the
/// `pickle` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(PICKLE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("pickle: exec namespace not a dict".into()))?;
    for name in PICKLE_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("pickle"));
            }
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
