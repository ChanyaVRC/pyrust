// `operator` module — body for the `operator` entry in
// `pyrust_builtin_modules!` (issue #2514).
//
// CPython's `operator` exports functions corresponding to the intrinsic
// operators (`add`, `sub`, `lt`, `getitem`, …), the in-place variants
// (`iadd`, …), and the generalized lookup objects (`itemgetter`,
// `attrgetter`, `methodcaller`).  Every one of these is most naturally — and
// most faithfully — expressed in Python itself: `operator.add(a, b)` is
// literally `a + b`, so routing through the interpreter's own operator
// machinery guarantees parity (BigInt promotion, user `__add__`/`__lt__`
// dispatch, sequence concat, error wording) for free.
//
// We therefore ship `operator` as a pure-Python module (`operator_py.py`,
// a close port of CPython 3.12's `Lib/operator.py`) exec'd once into a
// throwaway namespace at first import; the public names are copied onto the
// module by `inject_python_members`, wired from `env.rs::load_module`'s
// post-import hook (mirrors `collections` / `asyncio`, issues #1884 / #1039).
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import operator` resolves to a real `PyModule`.
//
// Reference: <https://docs.python.org/3/library/operator.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::PyKey;
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `operator` member.  Exec'd once
/// at first import (see `inject_python_members`).
const OPERATOR_PY_SOURCE: &str = include_str!("operator_py.py");

/// Public names from `OPERATOR_PY_SOURCE` exported onto the `operator` module.
/// Matches CPython 3.12's `operator.__all__` exactly.
const OPERATOR_PY_EXPORTS: [&str; 55] = [
    "abs",
    "add",
    "and_",
    "attrgetter",
    "call",
    "concat",
    "contains",
    "countOf",
    "delitem",
    "eq",
    "floordiv",
    "ge",
    "getitem",
    "gt",
    "iadd",
    "iand",
    "iconcat",
    "ifloordiv",
    "ilshift",
    "imatmul",
    "imod",
    "imul",
    "index",
    "indexOf",
    "inv",
    "invert",
    "ior",
    "ipow",
    "irshift",
    "is_",
    "is_not",
    "isub",
    "itemgetter",
    "itruediv",
    "ixor",
    "le",
    "length_hint",
    "lshift",
    "lt",
    "matmul",
    "methodcaller",
    "mod",
    "mul",
    "ne",
    "neg",
    "not_",
    "or_",
    "pos",
    "pow",
    "rshift",
    "setitem",
    "sub",
    "truediv",
    "truth",
    "xor",
];

/// Execute `OPERATOR_PY_SOURCE` once and copy its public names onto the
/// `operator` module's attribute map.  Called from the `operator` post-load
/// hook in `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(OPERATOR_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("operator: exec namespace not a dict".into()))?;
    for name in OPERATOR_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(())
}

pyrust_module! {}
