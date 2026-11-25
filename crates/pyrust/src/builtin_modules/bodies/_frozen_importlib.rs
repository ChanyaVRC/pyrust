// `_frozen_importlib` module — thin 100% native-Rust shim exposing CPython
// 3.12's core bootstrap import machinery (`ModuleSpec`, `BuiltinImporter`,
// `FrozenImporter`, `spec_from_loader`, `module_from_spec`, `_find_spec`,
// `_init_module_attrs`).

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::PyModule;

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let machinery = interp.load_module("importlib.machinery")?;
    for name in ["ModuleSpec", "BuiltinImporter", "FrozenImporter"] {
        if let Ok(val) = interp.get_attr(&machinery, name) {
            module.borrow_mut().insert_attr(name.to_string(), val);
        }
    }
    let util = interp.load_module("importlib.util")?;
    for name in [
        "spec_from_loader",
        "module_from_spec",
        "_find_spec",
        "_init_module_attrs",
    ] {
        if let Ok(val) = interp.get_attr(&util, name) {
            module.borrow_mut().insert_attr(name.to_string(), val);
        }
    }
    Ok(())
}

pyrust_module! {}
