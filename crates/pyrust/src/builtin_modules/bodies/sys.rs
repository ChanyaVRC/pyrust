// `sys` module — included into `pub mod sys { … }` declared by the
// `pyrust_builtin_modules!` invocation in
// `builtin_modules/mod.rs`.  `MODULE_NAME` is injected from
// the outer scope; no name literal appears in this file.
//
// Reference: <https://docs.python.org/3/library/sys.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{
    instantiate_exception, lookup_name_in_module, reject_keyword_args_expanded,
};
use crate::value::{PyClass, PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "version"      => Value::string("PyRust 0.2"),
        "argv"         => Value::list(Vec::new()),
        // CPython: sys.maxsize — the largest positive integer that fits in
        // Py_ssize_t (equivalent to i64::MAX on 64-bit platforms, which is
        // the only target pyrust supports).
        // <https://docs.python.org/3/library/sys.html#sys.maxsize>
        "maxsize"      => Value::int(i64::MAX),
        // CPython: sys.platform — identifier for the OS.
        // <https://docs.python.org/3/library/sys.html#sys.platform>
        "platform"     => Value::string(sys_platform()),
        // CPython: sys.version_info — named tuple with major/minor/micro/
        // releaselevel/serial.  We return a simple PyInstance with those
        // fields set, which satisfies the common `sys.version_info.major`
        // access pattern.
        // <https://docs.python.org/3/library/sys.html#sys.version_info>
        "version_info" => make_version_info(),
    }

    /// CPython: sys.exit([arg]) — raises `SystemExit(arg)`.
    /// <https://docs.python.org/3/library/sys.html#sys.exit>
    fn exit(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
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

/// Return the platform string matching CPython's `sys.platform` for the
/// current compilation target.
const fn sys_platform() -> &'static str {
    #[cfg(target_os = "linux")]
    { "linux" }
    #[cfg(target_os = "windows")]
    { "win32" }
    #[cfg(target_os = "macos")]
    { "darwin" }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    { "unknown" }
}

// The `sys.version_info` class singleton — a minimal named-tuple-like class
// whose instances expose `.major`, `.minor`, `.micro`, `.releaselevel`, and
// `.serial` as attributes.  Built once per thread; the singleton PyInstance
// is constructed from it in `make_version_info()`.
thread_local! {
    static VERSION_INFO_CLASS: Rc<RefCell<PyClass>> = Rc::new(RefCell::new(PyClass {
        name: "version_info".to_string(),
        qualname: "version_info".to_string(),
        base: None,
        attrs: indexmap::IndexMap::new(),
        mutation_version: std::cell::Cell::new(0),
    }));
}

/// Build the `sys.version_info` singleton value.  Called once per
/// `sys.module()` invocation (which the interpreter memoises in the module
/// cache).  Returns a `PyInstance` of `VERSION_INFO_CLASS` with the five
/// standard fields pre-set.
fn make_version_info() -> Value {
    VERSION_INFO_CLASS.with(|class| {
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        attrs.insert("major".to_string(), Value::int(3));
        // pyrust emulates Python 3.12 semantics.
        attrs.insert("minor".to_string(), Value::int(12));
        attrs.insert("micro".to_string(), Value::int(0));
        attrs.insert("releaselevel".to_string(), Value::string("final"));
        attrs.insert("serial".to_string(), Value::int(0));
        Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(class),
            attrs,
        })))
    })
}
