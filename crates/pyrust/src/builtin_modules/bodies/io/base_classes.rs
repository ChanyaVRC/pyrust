//! Python-source supplements and ABC re-parenting for the `io` module.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};

/// Python-source supplements for the `io` module (constants, `open` alias, and
/// the `IOBase` family of abstract base classes). Exec'd once at first import
/// (see `inject_python_members`).
const IO_PY_SOURCE: &str = include_str!("../io_py.py");

/// Public names from `IO_PY_SOURCE` copied onto the `io` module.
const IO_PY_EXPORTS: [&str; 9] = [
    "SEEK_SET",
    "SEEK_CUR",
    "SEEK_END",
    "DEFAULT_BUFFER_SIZE",
    "open",
    "IOBase",
    "RawIOBase",
    "BufferedIOBase",
    "TextIOBase",
];

/// Exec `IO_PY_SOURCE` once, copy its public names onto the `io` module, and
/// re-parent the native `BytesIO` / `StringIO` classes onto `BufferedIOBase` /
/// `TextIOBase` so the CPython isinstance hierarchy holds (issue #2778).
/// Called from the `io` post-load hook in `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(IO_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("io: exec namespace not a dict".into()))?;
    for name in IO_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            // The ABC classes render their repr / `__module__` via the exec
            // namespace's `__name__`, which defaults to `__main__`; override it
            // so `io.IOBase.__module__ == "io"` matches CPython.
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("io"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    // Re-parent the native concrete stream classes onto the matching ABC so
    // `isinstance(BytesIO(), BufferedIOBase)` / `isinstance(StringIO(),
    // TextIOBase)` (and transitively `IOBase`) hold. The macro builds both
    // with `base: None`, so this is the first parent assigned.
    for (concrete, abc) in [("BytesIO", "BufferedIOBase"), ("StringIO", "TextIOBase")] {
        let concrete_val = module.borrow().attrs.get(concrete).cloned();
        let abc_val = module.borrow().attrs.get(abc).cloned();
        if let (Some(c), Some(a)) = (concrete_val, abc_val)
            && let (ValueKind::PyClass(c_rc), ValueKind::PyClass(a_rc)) = (c.kind(), a.kind())
        {
            let error_name = match concrete {
                "BytesIO" => "_io.BytesIO",
                "StringIO" => "_io.StringIO",
                _ => unreachable!(),
            };
            let mut class = c_rc.borrow_mut();
            class.error_name = Some(error_name);
            class
                .attrs
                .insert("__module__".to_string(), Value::string("_io"));
            if class.base.is_none() {
                class.base = Some(Rc::clone(a_rc));
                drop(class);
                a_rc.borrow()
                    .subclasses
                    .borrow_mut()
                    .push(Rc::downgrade(c_rc));
            }
        }
    }
    Ok(Some(ns.clone()))
}
