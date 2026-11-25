// `sqlite3.dbapi2` submodule — body for the
// `"sqlite3.dbapi2" as sqlite3_dbapi2 @inject` entry in
// `pyrust_builtin_modules!` (issue #3225).
//
// Re-exports the DB-API 2.0 surface from `sqlite3` so both
// `import sqlite3.dbapi2` and `from sqlite3 import dbapi2` work cleanly.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::Value;
use pyrust_derive::pyrust_module;

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let parent_val = interp.load_module("sqlite3")?;
    if let crate::value::ValueKind::PyModule(parent_rc) = parent_val.kind() {
        let parent_attrs: Vec<(String, Value)> = parent_rc
            .borrow()
            .attrs
            .iter()
            .filter(|(k, _)| !k.starts_with("__") && k.as_str() != "dbapi2")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut mod_ref = module.borrow_mut();
        mod_ref
            .attrs
            .insert("__package__".to_string(), Value::string("sqlite3"));
        for (k, v) in parent_attrs {
            mod_ref.attrs.insert(k, v);
        }
        // Attach `dbapi2` onto the parent `sqlite3` module as well.
        parent_rc
            .borrow_mut()
            .attrs
            .insert("dbapi2".to_string(), Value::py_module(Rc::clone(module)));
    }
    Ok(None)
}

pyrust_module! {}
