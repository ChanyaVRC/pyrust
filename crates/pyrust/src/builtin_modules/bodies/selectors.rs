// `selectors` module — body for the `selectors @inject` entry in
// `pyrust_builtin_modules!` (issue #3205).
//
// High-level I/O multiplexing (`EVENT_READ`, `EVENT_WRITE`, `SelectorKey`,
// `BaseSelector`, `SelectSelector`, `DefaultSelector`) backed by the
// cross-platform OS readiness polling helper in `socket.rs` (`libc::poll` on
// Unix, Winsock2 `select` on Windows).
//
// Reference: <https://docs.python.org/3/library/selectors.html>

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyKey, Value, ValueKind};

/// Python-source definitions for the `selectors` module.
const SELECTORS_PY_SOURCE: &str = include_str!("selectors_py.py");

/// Private native helper moved into the exec namespace and hidden from
/// `dir(selectors)`.
const SELECTORS_NATIVE_HELPERS: &[&str] = &["_select_fds"];

/// Public attributes exported from `SELECTORS_PY_SOURCE` onto `selectors`.
const SELECTORS_PY_EXPORTS: &[&str] = &[
    "EVENT_READ",
    "EVENT_WRITE",
    "SelectorKey",
    "BaseSelector",
    "SelectSelector",
    "DefaultSelector",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for &helper in SELECTORS_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source_with_filename(
        SELECTORS_PY_SOURCE,
        "<frozen selectors>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("selectors: exec namespace not a dict".into()))?;
    for &name in SELECTORS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("selectors"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

fn extract_fd_list(val: &Value) -> Result<Vec<i64>> {
    let items: Vec<Value> = match val.kind() {
        ValueKind::List(items) => items.clone(),
        ValueKind::Tuple(rc) => rc.as_ref().to_vec(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("expected list of fds, not {}", value_type_name_str(val)),
            ));
        }
    };
    let mut fds = Vec::with_capacity(items.len());
    for item in items {
        match item.kind() {
            ValueKind::Int(n) => fds.push(n),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("fd must be int, not {}", value_type_name_str(&item)),
                ));
            }
        }
    }
    Ok(fds)
}

fn select_fds_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            "_select_fds() expects 3 arguments".to_string(),
        ));
    }
    let rlist = extract_fd_list(&args[0].value)?;
    let wlist = extract_fd_list(&args[1].value)?;
    let timeout = match args[2].value.kind() {
        ValueKind::None => None,
        ValueKind::Int(n) => {
            if n <= 0 {
                Some(Duration::ZERO)
            } else {
                Some(Duration::from_secs(n as u64))
            }
        }
        ValueKind::Float(f) => {
            if f.is_nan() {
                return Err(PyError::named(
                    "ValueError",
                    "timeout must not be NaN".to_string(),
                ));
            }
            if f <= 0.0 {
                Some(Duration::ZERO)
            } else if f.is_infinite() {
                None
            } else {
                Some(Duration::from_secs_f64(f))
            }
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "timeout must be a number or None, not {}",
                    value_type_name_str(&args[2].value)
                ),
            ));
        }
    };

    // Wait with the GIL released (CPython's `Py_BEGIN_ALLOW_THREADS` around
    // `select()`): `socketserver.serve_forever()` parks here for every poll
    // interval, and holding the GIL would starve every other Python thread.
    let (ready_r, ready_w) =
        interp.release_gil_during(|| super::socket::select_raw_handles(&rlist, &wlist, timeout))?;
    let r_vals = ready_r.into_iter().map(Value::int).collect();
    let w_vals = ready_w.into_iter().map(Value::int).collect();
    Ok(Value::tuple(vec![Value::list(r_vals), Value::list(w_vals)]))
}

pyrust_module! {
    fn _select_fds(args) -> Result<Value> {
        select_fds_impl(_interp, args)
    }
}
