//! Python-source member injection for the native `typing` module.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, PyModule, Value, ValueKind};

/// Python-source definitions for runtime helpers and special-form markers.
const TYPING_PY_SOURCE: &str = include_str!("../typing_py.py");

/// Public names copied from the throwaway execution namespace.
const TYPING_PY_EXPORTS: &[&str] = &[
    "get_origin",
    "get_args",
    "get_type_hints",
    "runtime_checkable",
    "final",
    "no_type_check",
    "reveal_type",
    "assert_never",
    "assert_type",
    "dataclass_transform",
    "get_overloads",
    "clear_overloads",
    "Self",
    "Never",
    "LiteralString",
    "Annotated",
    "TypeAlias",
    "Concatenate",
    "Unpack",
    "Required",
    "NotRequired",
    "TypeGuard",
    "ParamSpec",
    "ParamSpecArgs",
    "ParamSpecKwargs",
    "TypeVarTuple",
];

/// Execute `typing_py.py` and copy its supported names onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    for name in [
        "Optional", "Union", "Type", "Callable", "ClassVar", "Final", "Literal",
    ] {
        if let Some(value) = module.borrow().attrs.get(name).cloned() {
            ns.dict_insert(PyKey::str_from(name), value)?;
        }
    }

    // The macro-generated class only owns the `typing.TypeVar.__init__`
    // registry body.  Python-visible identity belongs to the runtime
    // singleton so PEP 695 parameters, manual construction, retained
    // factories, and a re-imported `typing` module all agree.
    module.borrow_mut().insert_attr(
        "TypeVar".to_string(),
        Value::py_class(crate::interpreter::typevar_class_singleton()),
    );

    module.borrow_mut().insert_attr(
        "TypeAliasType".to_string(),
        Value::py_class(crate::interpreter::type_alias_class_singleton()),
    );

    interp.exec_source(TYPING_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("typing: exec namespace not a dict".into()))?;
    let mut exports = TYPING_PY_EXPORTS.to_vec();
    exports.extend([
        "_namedtuple_functional",
        "_build_namedtuple_class",
        "_typeddict_functional",
        "_build_typeddict_class",
    ]);

    for name in exports {
        if let Some(value) = dict.get(&PyKey::str_from(name)) {
            if matches!(name, "ParamSpecArgs" | "ParamSpecKwargs")
                && let ValueKind::PyClass(class) = value.kind()
            {
                class
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("typing"));
            }
            module
                .borrow_mut()
                .insert_attr(name.to_string(), value.clone());
        }
    }
    Ok(())
}
