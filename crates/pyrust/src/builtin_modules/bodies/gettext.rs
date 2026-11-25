// `gettext` module — body for the `gettext @inject` entry in
// `pyrust_builtin_modules!` (issue #3226).
//
// Pure-Python implementation of Python 3.12's `gettext` module
// (`gettext_py.py`): safe recursive-descent GNU plural expression compiler
// (`c2py`), `NullTranslations`, `GNUTranslations` with full little-endian and
// big-endian binary `.mo` parser (contexts, plurals, headers, `Plural-Forms`,
// `Content-Type` charset), `find`, `translation`/`Catalog`, `install`, and
// the global domain API (`bindtextdomain`, `textdomain`, `*gettext`).
//
// Reference: <https://docs.python.org/3.12/library/gettext.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `gettext` module.
const GETTEXT_PY_SOURCE: &str = include_str!("gettext_py.py");

/// Public and compatibility names exported from `GETTEXT_PY_SOURCE` onto `gettext`.
const GETTEXT_PY_EXPORTS: &[&str] = &[
    "__all__",
    "c2py",
    "_expand_lang",
    "_default_localedir",
    "NullTranslations",
    "GNUTranslations",
    "Catalog",
    "find",
    "translation",
    "install",
    "bindtextdomain",
    "textdomain",
    "dgettext",
    "dngettext",
    "gettext",
    "ngettext",
    "pgettext",
    "dpgettext",
    "npgettext",
    "dnpgettext",
];

/// Execute `GETTEXT_PY_SOURCE` once and publish its exports onto `gettext`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(
        GETTEXT_PY_SOURCE,
        "<frozen gettext>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("gettext: exec namespace not a dict".into()))?;
    for &name in GETTEXT_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("gettext"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    drop(dict);
    Ok(Some(ns))
}

pyrust_module! {}
