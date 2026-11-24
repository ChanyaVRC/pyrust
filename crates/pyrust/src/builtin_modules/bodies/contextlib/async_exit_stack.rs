//! Python-source member injection for `contextlib` (`AbstractContextManager`,
//! `AbstractAsyncContextManager`, `ContextDecorator`, `AsyncContextDecorator`,
//! `asynccontextmanager`, `aclosing`, `AsyncExitStack`).

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};

const CONTEXTLIB_PY_SOURCE: &str = include_str!("../contextlib_py.py");

/// Public names defined by `CONTEXTLIB_PY_SOURCE` to export onto the module.
/// `_check_methods` and `_AsyncGeneratorContextManager` are private helpers and
/// are intentionally omitted.
const CONTEXTLIB_PY_EXPORTS: [&str; 7] = [
    "AbstractContextManager",
    "AbstractAsyncContextManager",
    "ContextDecorator",
    "AsyncContextDecorator",
    "asynccontextmanager",
    "aclosing",
    "AsyncExitStack",
];

/// Exec `CONTEXTLIB_PY_SOURCE` once and copy its public names onto the
/// `contextlib` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    // Seed the exec namespace with the names the Python source references at
    // class-definition time (`ABC`, `abstractmethod`, `wraps`, `GenericAlias`).
    seed_dependency(interp, &ns, "abc", &["ABC", "abstractmethod"])?;
    seed_dependency(interp, &ns, "functools", &["wraps"])?;
    seed_dependency(interp, &ns, "types", &["GenericAlias"])?;
    interp.exec_source(CONTEXTLIB_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("contextlib: exec namespace not a dict".into()))?;
    for name in CONTEXTLIB_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
            if let ValueKind::PyClass(class) = val.kind() {
                class
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("contextlib"));
            }
        }
    }
    Ok(Some(ns.clone()))
}

fn seed_dependency(
    interp: &mut Interpreter,
    ns: &Value,
    module_name: &str,
    names: &[&str],
) -> Result<()> {
    let module = interp.load_module(module_name)?;
    let ValueKind::PyModule(m) = module.kind() else {
        return Err(PyError::runtime(format!(
            "contextlib: {module_name} is not a module",
        )));
    };
    for name in names {
        let val = m
            .borrow()
            .get_attr_value(name)
            .ok_or_else(|| PyError::runtime(format!("contextlib: {module_name}.{name} missing")))?;
        ns.dict_insert(PyKey::str_from(name), val)?;
    }
    Ok(())
}
