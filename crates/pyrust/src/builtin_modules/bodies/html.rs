// `html` package module — body for the `html @inject` entry in
// `pyrust_builtin_modules!` (issue #3149).
//
// Exposes `escape` and `unescape` from `html_py.py` alongside the `entities`
// and `parser` submodules. Declaring `entities` and `parser` as `PyModule`
// constants lets `Interpreter::load_module` stitch them to the cached
// `sys.modules["html.entities"]` and `sys.modules["html.parser"]` objects so
// `import html`, `import html.entities`, `import html.parser`, and
// `from html.parser import HTMLParser` share exact module identity.
//
// Reference: <https://docs.python.org/3/library/html.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `html` module.
const HTML_PY_SOURCE: &str = include_str!("html_py.py");

/// Public and internal names exported from `HTML_PY_SOURCE` onto `html`.
const HTML_PY_EXPORTS: [&str; 7] = [
    "__all__",
    "escape",
    "unescape",
    "_invalid_charrefs",
    "_invalid_codepoints",
    "_replace_charref",
    "_charref",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(HTML_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("html: exec namespace not a dict".into()))?;
    for name in HTML_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {
    constants {
        "entities" => super::html_entities::module(),
        "parser" => super::html_parser::module(),
    }
}
