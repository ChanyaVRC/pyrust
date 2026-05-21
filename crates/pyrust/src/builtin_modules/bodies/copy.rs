// `copy` module — CPython-compatible `copy` module stub.
//
// Currently exports `copy.Error`, a subclass of `Exception` used when a
// deepcopy operation fails on an unsupported type.  See CPython's
// `Lib/copy.py` for the reference implementation.
//
// ## `copy.Error` design
//
// `copy.Error` is a plain exception class subclassing `Exception`.  We build
// it via a thread-local `Rc<RefCell<PyClass>>` (the same pattern used by
// `os.rs`'s `ENVIRON_CLASS` and `typing.rs`'s `ANY_CLASS`) rather than the
// `pyrust_module!` `class { }` block, because the macro-generated class block
// always sets `base: None`.  Setting `base` to the `Exception` singleton is
// essential so:
//  - `is_exception_class` returns `true` (the `raise`/`except` machinery
//    calls this),
//  - `issubclass(copy.Error, Exception)` returns `True`,
//  - `isinstance(copy.Error(), Exception)` returns `True`.
//
// The `Exception` class Rc is fetched from the thread-local `EXC_CLASS_CACHE`
// via `crate::interpreter::lookup_exc_class("Exception")`.  That cache is
// already initialised long before any `import copy` runs — it's built lazily
// on the first exception raise or class lookup — but even if it isn't yet
// populated, `lookup_exc_class` triggers its initialisation, so the result is
// always `Some(...)`.
//
// Reference: <https://docs.python.org/3/library/copy.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::value::{PyClass, Value};
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

// ── thread-local Error class singleton ───────────────────────────────────────
//
// Built once per interpreter thread.  Cloned (O(1) Rc bump) into the module
// attrs map on every `module()` call (which the import cache calls at most
// once per process — see `load_builtin_module`).

thread_local! {
    static COPY_ERROR_CLASS: Rc<RefCell<PyClass>> = {
        // Fetch the `Exception` base class from the per-thread exception
        // hierarchy cache.  `lookup_exc_class` triggers initialisation of
        // that cache if it hasn't happened yet.
        let exception_base = crate::interpreter::lookup_exc_class("Exception")
            .expect("EXC_CLASS_CACHE must contain Exception");
        let mut attrs = IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("copy".to_string()));
        Rc::new(RefCell::new(PyClass {
            name: "Error".to_string(),
            qualname: "Error".to_string(),
            base: Some(exception_base),
            attrs,
        }))
    };
}

/// Return the thread-local `copy.Error` class as a `Value::py_class`.
fn copy_error_class_value() -> Value {
    COPY_ERROR_CLASS.with(|c| Value::py_class(Rc::clone(c)))
}

pyrust_module! {
    constants {
        // `copy.Error` — subclass of `Exception`.  Exposed as a module
        // attribute so `import copy; copy.Error` resolves correctly.
        "Error" => copy_error_class_value(),
        // CPython's `copy.py` keeps a lowercase alias for backward
        // compatibility: `error = Error`.  Both names must resolve to the
        // same class so `copy.error is copy.Error` is `True`.
        "error" => copy_error_class_value(),
    }
}
