//! Python-source member injection for the native `typing` module.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, PyModule, Value};

/// Python-source definitions for runtime helpers and special-form markers.
const TYPING_PY_SOURCE: &str = include_str!("../typing_py.py");

/// Public names copied from the throwaway execution namespace.
const TYPING_PY_EXPORTS: &[&str] = &[
    "get_origin",
    "get_args",
    "get_type_hints",
    "runtime_checkable",
    "final",
    "override",
    "no_type_check",
    "no_type_check_decorator",
    "reveal_type",
    "assert_never",
    "assert_type",
    "dataclass_transform",
    "overload",
    "get_overloads",
    "clear_overloads",
    "is_typeddict",
    "NewType",
    "Self",
    "Never",
    "NoReturn",
    "LiteralString",
    "Annotated",
    "TypeAlias",
    "Concatenate",
    "Unpack",
    "Required",
    "NotRequired",
    "TypeGuard",
    "_UnpackGenericAlias",
];

/// Execute `typing_py.py` and copy its supported names onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<Option<Value>> {
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
    // singletons so PEP 695 parameters, manual construction, retained
    // factories, and a re-imported `typing` module all agree.
    let typevar_cls = Value::py_class(crate::interpreter::typevar_class_singleton());
    let paramspec_cls = Value::py_class(crate::interpreter::paramspec_class_singleton());
    let paramspec_args_cls = Value::py_class(crate::interpreter::paramspec_args_class_singleton());
    let paramspec_kwargs_cls =
        Value::py_class(crate::interpreter::paramspec_kwargs_class_singleton());
    let typevartuple_cls = Value::py_class(crate::interpreter::typevartuple_class_singleton());
    let type_alias_cls = Value::py_class(crate::interpreter::type_alias_class_singleton());

    {
        let mut mod_ref = module.borrow_mut();
        mod_ref.insert_attr("TypeVar".to_string(), typevar_cls.clone());
        mod_ref.insert_attr("ParamSpec".to_string(), paramspec_cls.clone());
        mod_ref.insert_attr("ParamSpecArgs".to_string(), paramspec_args_cls);
        mod_ref.insert_attr("ParamSpecKwargs".to_string(), paramspec_kwargs_cls);
        mod_ref.insert_attr("TypeVarTuple".to_string(), typevartuple_cls.clone());
        mod_ref.insert_attr("TypeAliasType".to_string(), type_alias_cls);
    }

    ns.dict_insert(PyKey::str_from("TypeVar"), typevar_cls)?;
    ns.dict_insert(PyKey::str_from("ParamSpec"), paramspec_cls)?;
    ns.dict_insert(PyKey::str_from("TypeVarTuple"), typevartuple_cls)?;

    interp.exec_source(TYPING_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("typing: exec namespace not a dict".into()))?;
    let mut exports = TYPING_PY_EXPORTS.to_vec();
    exports.extend([
        "_namedtuple_functional",
        "_build_namedtuple_class",
        "_typeddict_functional",
        "_build_typeddict_class",
    ]);

    for name in exports {
        if let Some(value) = dict.get(&PyKey::str_from(name)) {
            if name == "Annotated" {
                super::generation::register_annotated_marker(value);
            }
            module
                .borrow_mut()
                .insert_attr(name.to_string(), value.clone());
        }
    }
    Ok(Some(ns.clone()))
}
