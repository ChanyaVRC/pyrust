// `importlib.simple` compatibility shim — 100% native-Rust re-export of
// `importlib.resources.simple` (`SimpleReader`, `ResourceHandle`,
// `ResourceContainer`, `TraversableReader`).

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
    let simple = interp.load_module("importlib.resources.simple")?;
    for name in [
        "SimpleReader",
        "ResourceHandle",
        "ResourceContainer",
        "TraversableReader",
    ] {
        if let Ok(val) = interp.get_attr(&simple, name) {
            module.borrow_mut().insert_attr(name.to_string(), val);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("SimpleReader"),
            Value::string("ResourceHandle"),
            Value::string("ResourceContainer"),
            Value::string("TraversableReader"),
        ]),
    }
}
