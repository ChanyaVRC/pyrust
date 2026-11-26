// `curses.panel` module — 100% native Rust wrapper re-exporting `_curses_panel`
// with exact object identity (issue #3266).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs` to match CPython 3.12.
// Reference: <https://docs.python.org/3.12/library/curses.panel.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::{PyModule, Value, ValueKind};

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
    let cp_val = interp.load_module("_curses_panel")?;
    if let ValueKind::PyModule(src_rc) = cp_val.kind() {
        let snapshot = src_rc.borrow().attrs_snapshot();
        let mut dst = module.borrow_mut();
        for (k, v) in snapshot {
            if SKIP_DUNDERS.contains(&k.as_str()) {
                continue;
            }
            dst.insert_attr(k, v);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("curses"),
    }
}
