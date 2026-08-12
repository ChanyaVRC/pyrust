// `decimal` module — body for the `decimal` entry in
// `pyrust_builtin_modules!` (issue #2806).
//
// CPython ships a fast C `_decimal` extension with a pure-Python fallback
// (`Lib/_pydecimal.py`).  pyrust has no C extension, so we port the pure-Python
// reference implementation verbatim into `decimal_py.py` and exec it once into a
// throwaway namespace at first `import decimal`; the public names are copied
// onto the module by `inject_python_members` (mirrors `json` / `dataclasses`).
//
// The upstream `_pydecimal.py` pulls in a few modules pyrust does not ship
// (`numbers`, `contextvars`, optionally `locale`).  Those are stubbed inside
// `decimal_py.py` itself (see the `pyrust note:` comments there).  The
// dependencies pyrust *does* ship (`math`, `re`, `collections`, `sys`,
// `itertools`) are imported at the top of the Python source and resolve through
// the normal `import` machinery in the exec namespace.
//
// The native `pyrust_module!` block is intentionally empty — it exists only so
// the `pyrust_builtin_modules!` plumbing (`module()` / `regs()`) is generated
// and `import decimal` resolves to a real `PyModule`.
//
// Reference: <https://docs.python.org/3/library/decimal.html>
// Source basis: CPython 3.12 `Lib/_pydecimal.py`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyDict, PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for every public `decimal` member.  Exec'd once at
/// first import (see `inject_python_members`).
const DECIMAL_PY_SOURCE: &str = include_str!("decimal_py.py");

/// Public names from `DECIMAL_PY_SOURCE` exported onto the `decimal` module.
/// Mirrors CPython 3.12's `decimal.__all__`.
const DECIMAL_PY_EXPORTS: &[&str] = &[
    // Two major classes
    "Decimal",
    "Context",
    // Named tuple representation
    "DecimalTuple",
    // Contexts
    "DefaultContext",
    "BasicContext",
    "ExtendedContext",
    // Exceptions
    "DecimalException",
    "Clamped",
    "InvalidOperation",
    "DivisionByZero",
    "Inexact",
    "Rounded",
    "Subnormal",
    "Overflow",
    "Underflow",
    "FloatOperation",
    // Exceptional conditions that trigger InvalidOperation
    "DivisionImpossible",
    "InvalidContext",
    "ConversionSyntax",
    "DivisionUndefined",
    // Constants for use in setting up contexts
    "ROUND_DOWN",
    "ROUND_HALF_UP",
    "ROUND_HALF_EVEN",
    "ROUND_CEILING",
    "ROUND_FLOOR",
    "ROUND_UP",
    "ROUND_HALF_DOWN",
    "ROUND_05UP",
    // Functions for manipulating contexts
    "setcontext",
    "getcontext",
    "localcontext",
    // Limits for compatibility with the C version
    "MAX_PREC",
    "MAX_EMAX",
    "MIN_EMIN",
    "MIN_ETINY",
    // C-version compile-time flags (always true here)
    "HAVE_THREADS",
    "HAVE_CONTEXTVAR",
];

/// Classes whose `__module__` must read `decimal` (their repr / qualname and
/// exception rendering depend on it).  The exec namespace's `__name__` defaults
/// to `__main__`, so we override it here for the public classes/exceptions.
const DECIMAL_PY_CLASS_NAMES: &[&str] = &[
    "Decimal",
    "Context",
    "DecimalException",
    "Clamped",
    "InvalidOperation",
    "DivisionByZero",
    "Inexact",
    "Rounded",
    "Subnormal",
    "Overflow",
    "Underflow",
    "FloatOperation",
    "DivisionImpossible",
    "InvalidContext",
    "ConversionSyntax",
    "DivisionUndefined",
];

/// Execute `DECIMAL_PY_SOURCE` once and copy its public names onto the
/// `decimal` module's attribute map.  Called from the `@inject` post-load hook
/// in `mod.rs::post_load_inject`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = Value::dict(PyDict::default());
    // `decimal_py.py` reads `__name__` (it does `__xname__ = __name__` before
    // overriding `__name__ = 'decimal'`).  The exec namespace has no implicit
    // `__name__`, so seed it.
    ns.dict_insert(PyKey::str_from("__name__"), Value::string("decimal"))?;
    // Pre-seed module dependencies.  `import` statements executed through this
    // injection path do not bind their target names in the exec namespace, so
    // seed every module `decimal_py.py` references at runtime under the name the
    // source uses for it.  The source's own `import ... [as ...]` statements
    // (and its later `del sys` / `del re`) then operate on these bindings.
    for (alias, module_name) in [
        ("sys", "sys"),
        ("_math", "math"),
        ("re", "re"),
        ("_collections", "collections"),
        ("_itertools", "itertools"),
    ] {
        let m = interp.load_module(module_name)?;
        ns.dict_insert(PyKey::str_from(alias), m)?;
    }
    interp.exec_source(DECIMAL_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("decimal: exec namespace not a dict".into()))?;
    for name in DECIMAL_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if DECIMAL_PY_CLASS_NAMES.contains(name)
                && let ValueKind::PyClass(cls_rc) = val.kind()
            {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("decimal"));
                if *name == "Decimal" {
                    class.error_name = Some("decimal.Decimal");
                }
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
