// `importlib.readers` compatibility shim — 100% native-Rust re-export of
// `importlib.resources.readers` (`FileReader`, `ZipReader`, `MultiplexedPath`,
// `NamespaceReader`).

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::{PyModule, Value};

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let readers = interp.load_module("importlib.resources.readers")?;
    for name in [
        "FileReader",
        "ZipReader",
        "MultiplexedPath",
        "NamespaceReader",
    ] {
        if let Ok(val) = interp.get_attr(&readers, name) {
            module.borrow_mut().insert_attr(name.to_string(), val);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("FileReader"),
            Value::string("ZipReader"),
            Value::string("MultiplexedPath"),
            Value::string("NamespaceReader"),
        ]),
    }
}
