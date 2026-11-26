// `xml.parsers.expat` module — 100% native-Rust Python 3.12 wrapper re-exporting
// `pyexpat` and registering `xml.parsers.expat.errors` and `xml.parsers.expat.model`
// in `sys.modules` with exact object identity (issue #3260).
//
// Reference: <https://docs.python.org/3/library/pyexpat.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::{PyKey, PyModule, Value, ValueKind};

const SKIP_DUNDERS: &[&str] = &[
    "__name__",
    "__doc__",
    "__package__",
    "__loader__",
    "__spec__",
    "__file__",
    "__path__",
    "__all__",
    "__cached__",
    "__builtins__",
];

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let pyexpat_val = interp.load_module("pyexpat")?;
    if let ValueKind::PyModule(src_rc) = pyexpat_val.kind() {
        let snapshot = src_rc.borrow().attrs_snapshot();
        let mut dst = module.borrow_mut();
        for (k, v) in snapshot {
            if SKIP_DUNDERS.contains(&k.as_str()) {
                continue;
            }
            dst.insert_attr(k, v);
        }
    }
    let errors_mod = interp.get_attr(&pyexpat_val, "errors")?;
    let model_mod = interp.get_attr(&pyexpat_val, "model")?;
    let sys_mod = interp.load_module("sys")?;
    let modules = interp.get_attr(&sys_mod, "modules")?;
    let _ = modules.dict_insert(
        PyKey::str_from("xml.parsers.expat.errors"),
        errors_mod.clone(),
    );
    let _ = modules.dict_insert(
        PyKey::str_from("xml.parsers.expat.model"),
        model_mod.clone(),
    );
    module.borrow_mut().insert_attr("sys".to_string(), sys_mod);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.parsers"),
    }
}
