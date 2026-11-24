// `struct` module — body for the `"struct" as struct_mod @inject` entry in
// `pyrust_builtin_modules!` (issue #2807).
//
// This file is the thin Python-registration and injection facade. Format
// parsing, coercion/binary16/offset validation, packing, and unpacking live in
// submodules under `bodies/struct_mod/`.
//
// Reference: <https://docs.python.org/3/library/struct.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

#[path = "struct_mod/coercion.rs"]
mod coercion;
#[path = "struct_mod/format.rs"]
mod format;
#[path = "struct_mod/pack.rs"]
mod pack;
#[path = "struct_mod/unpack.rs"]
mod unpack;

/// Python-source definitions for the public `struct` surface (`error`,
/// `Struct`, top-level wrappers, `_clearcache`, `__all__`).
const STRUCT_PY_SOURCE: &str = include_str!("struct_mod_py.py");

/// Private native helpers seeded into the injected exec namespace and removed
/// from `struct.__dict__` after injection so `dir(struct)` matches CPython.
const STRUCT_NATIVE_HELPERS: [&str; 6] = [
    "_parse_format",
    "_pack",
    "_unpack",
    "_pack_into",
    "_unpack_from",
    "_check_iter_unpack",
];

/// Public names exported from `STRUCT_PY_SOURCE` onto the `struct` module.
const STRUCT_PY_EXPORTS: [&str; 9] = [
    "error",
    "Struct",
    "calcsize",
    "pack",
    "unpack",
    "pack_into",
    "unpack_from",
    "iter_unpack",
    "_clearcache",
];

/// Execute `STRUCT_PY_SOURCE` once, wire its closures to the native primitives,
/// hide the internal helper names from `dir(struct)`, and export the public
/// CPython 3.12 `struct` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in STRUCT_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(STRUCT_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("struct: exec namespace not a dict".into()))?;
    for name in STRUCT_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("struct"));
                if name == "Struct" {
                    class.error_name = Some("struct.Struct");
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

pyrust_module! {
    /// `_parse_format(error_cls, format)` -> `(normalized_str, size)`.
    fn _parse_format(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let compiled = format::compile_format(&args[1].value, error_cls)?;
        Ok(Value::tuple(vec![
            Value::string(&compiled.format_str),
            Value::int(compiled.size as i64),
        ]))
    }

    /// `_pack(error_cls, format, values_tuple)` -> `bytes`.
    fn _pack(args) -> Result<Value> {
        pack::pack_entry(_interp, FN_NAME, args)
    }

    /// `_unpack(error_cls, format, buffer)` -> `tuple`.
    fn _unpack(args) -> Result<Value> {
        unpack::unpack_entry(_interp, FN_NAME, args)
    }

    /// `_pack_into(error_cls, format, buffer, offset, values_tuple)` -> `None`.
    fn _pack_into(args) -> Result<Value> {
        pack::pack_into_entry(_interp, FN_NAME, args)
    }

    /// `_unpack_from(error_cls, format, buffer, offset)` -> `tuple`.
    fn _unpack_from(args) -> Result<Value> {
        unpack::unpack_from_entry(_interp, FN_NAME, args)
    }

    /// `_check_iter_unpack(error_cls, format, buffer)` -> `(size, buf_len)`.
    fn _check_iter_unpack(args) -> Result<Value> {
        unpack::check_iter_unpack_entry(_interp, FN_NAME, args)
    }
}
