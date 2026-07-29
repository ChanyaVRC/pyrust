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
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `operator` member.  Exec'd once
/// at first import (see `inject_python_members`).
const OPERATOR_PY_SOURCE: &str = include_str!("operator_py.py");

/// Public names from `OPERATOR_PY_SOURCE` exported onto the `operator` module.
///
/// Matches CPython 3.12's `operator.__all__` minus `length_hint`: CPython's
/// `Lib/operator.py` ends with `from _operator import *`, so the accelerated C
/// definitions shadow the Python ones, and `length_hint` is the one member
/// whose two implementations observably differ (issue #2920).  It is declared
/// natively below instead, and must not be overwritten from the Python
/// namespace here.
const OPERATOR_PY_EXPORTS: [&str; 54] = [
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

pyrust_module! {
    /// CPython: operator.length_hint(obj, default=0) → int.
    /// <https://docs.python.org/3/library/operator.html#operator.length_hint>
    ///
    /// The accelerated `_operator.length_hint` (which shadows the Python
    /// definition in `Lib/operator.py`) is a thin wrapper over
    /// `PyObject_LengthHint`, so this body only parses the argument list and
    /// hands off to the interpreter's shared hint protocol
    /// (`Interpreter::object_length_hint`).  Keeping the protocol there is
    /// what lets a built-in iterator answer from its own cursor rather than
    /// from a special case here (issue #2920).
    ///
    /// The legacy argument dialect is deliberate: CPython's C signature is
    /// positional-only with `METH_VARARGS` arity wording that the typed
    /// prelude's `_operator.`-qualified keyword rejection cannot reproduce.
    fn length_hint(args) -> Result<Value> {
        if args.iter().any(|arg| arg.name.is_some()) {
            return Err(PyError::named(
                "TypeError",
                "_operator.length_hint() takes no keyword arguments".to_string(),
            ));
        }
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                "length_hint expected at least 1 argument, got 0".to_string(),
            ));
        }
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "length_hint expected at most 2 arguments, got {}",
                    args.len()
                ),
            ));
        }
        // `default` is a plain `Py_ssize_t` in the C signature: it goes through
        // `__index__` and may legitimately be negative, so it does not share
        // the `>= 0` validation the hint result gets.
        let default = match args.get(1) {
            Some(arg) => _interp.value_to_isize(
                &arg.value,
                "Python int too large to convert to C ssize_t",
            )?,
            None => 0,
        };
        _interp
            .object_length_hint(&args[0].value, default)
            .map(Value::int)
    }
}
