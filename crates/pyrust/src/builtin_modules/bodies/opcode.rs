// `opcode` module — body for the `opcode @inject` entry in
// `pyrust_builtin_modules!` (issue #3227).
//
// CPython 3.12's `opcode` module defines the Python 3.12 bytecode opcode map,
// classification lists (`hasarg`, `hasconst`, `hasname`, `hasjrel`, `hasjabs`,
// `haslocal`, `hascompare`, `hasfree`, `hasexc`), comparison operators
// (`cmp_op`), `HAVE_ARGUMENT`, `EXTENDED_ARG`, and `stack_effect()`.
//
// Reference: <https://docs.python.org/3/library/dis.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `opcode` module.
const OPCODE_PY_SOURCE: &str = include_str!("opcode_py.py");

/// Public and internal names exported from `OPCODE_PY_SOURCE` onto `opcode`
/// (matching CPython 3.12's `Lib/opcode.py` so `dis` and user code find every
/// standard attribute).
const OPCODE_PY_EXPORTS: &[&str] = &[
    "__doc__",
    "__all__",
    "cmp_op",
    "hasarg",
    "hasconst",
    "hasname",
    "hasjrel",
    "hasjabs",
    "haslocal",
    "hascompare",
    "hasfree",
    "hasexc",
    "opname",
    "opmap",
    "HAVE_ARGUMENT",
    "EXTENDED_ARG",
    "stack_effect",
    "ENABLE_SPECIALIZATION",
    "MIN_INSTRUMENTED_OPCODE",
    "MIN_PSEUDO_OPCODE",
    "MAX_PSEUDO_OPCODE",
    "is_pseudo",
    "oplists",
    "_pseudo_ops",
    "_nb_ops",
    "_intrinsic_1_descs",
    "_intrinsic_2_descs",
    "_specializations",
    "_specialized_instructions",
    "_cache_format",
    "_inline_cache_entries",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(OPCODE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("opcode: exec namespace not a dict".into()))?;
    for name in OPCODE_PY_EXPORTS {
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
