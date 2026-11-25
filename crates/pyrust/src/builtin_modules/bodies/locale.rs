// `locale` module — body for the `locale @inject` entry in
// `pyrust_builtin_modules!` (issue #3226).
//
// Platform-matching `LC_*` / `CHAR_MAX` constants exported natively from
// `locale.rs` (using `libc::LC_*` on Unix and MSVC CRT values on Windows —
// zero extra C-sys dependencies), paired with the full pure-Python
// internationalization, localeconv, formatting, parsing, and normalization
// implementation in `locale_py.py`.
//
// Reference: <https://docs.python.org/3.12/library/locale.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `locale` module.
const LOCALE_PY_SOURCE: &str = include_str!("locale_py.py");

/// Public names exported from `LOCALE_PY_SOURCE` onto `locale`.
const LOCALE_PY_EXPORTS: &[&str] = &[
    "__all__",
    "Error",
    "locale_alias",
    "encoding_alias",
    "windows_locale",
    "_override_localeconv",
    "normalize",
    "setlocale",
    "getlocale",
    "resetlocale",
    "getdefaultlocale",
    "getencoding",
    "getpreferredencoding",
    "localeconv",
    "format_string",
    "currency",
    "str",
    "delocalize",
    "localize",
    "atof",
    "atoi",
    "strcoll",
    "strxfrm",
];

#[cfg(unix)]
const HOST_LC_ALL: i64 = libc::LC_ALL as i64;
#[cfg(unix)]
const HOST_LC_CTYPE: i64 = libc::LC_CTYPE as i64;
#[cfg(unix)]
const HOST_LC_NUMERIC: i64 = libc::LC_NUMERIC as i64;
#[cfg(unix)]
const HOST_LC_TIME: i64 = libc::LC_TIME as i64;
#[cfg(unix)]
const HOST_LC_COLLATE: i64 = libc::LC_COLLATE as i64;
#[cfg(unix)]
const HOST_LC_MONETARY: i64 = libc::LC_MONETARY as i64;
#[cfg(unix)]
const HOST_LC_MESSAGES: i64 = libc::LC_MESSAGES as i64;

#[cfg(windows)]
const HOST_LC_ALL: i64 = 0;
#[cfg(windows)]
const HOST_LC_COLLATE: i64 = 1;
#[cfg(windows)]
const HOST_LC_CTYPE: i64 = 2;
#[cfg(windows)]
const HOST_LC_MONETARY: i64 = 3;
#[cfg(windows)]
const HOST_LC_NUMERIC: i64 = 4;
#[cfg(windows)]
const HOST_LC_TIME: i64 = 5;

#[cfg(not(any(unix, windows)))]
const HOST_LC_ALL: i64 = 0;
#[cfg(not(any(unix, windows)))]
const HOST_LC_COLLATE: i64 = 1;
#[cfg(not(any(unix, windows)))]
const HOST_LC_CTYPE: i64 = 2;
#[cfg(not(any(unix, windows)))]
const HOST_LC_MONETARY: i64 = 3;
#[cfg(not(any(unix, windows)))]
const HOST_LC_NUMERIC: i64 = 4;
#[cfg(not(any(unix, windows)))]
const HOST_LC_TIME: i64 = 5;

/// Execute `LOCALE_PY_SOURCE` once and publish its exports onto `locale`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    // Seed native constants into the exec namespace so `locale_py.py` uses the
    // exact platform LC_* integer values.
    #[cfg(unix)]
    {
        module
            .borrow_mut()
            .attrs
            .insert("LC_MESSAGES".to_string(), Value::int(HOST_LC_MESSAGES));
    }
    {
        let module_ref = module.borrow();
        for (k, v) in &module_ref.attrs {
            ns.dict_insert(PyKey::str_from(k), v.clone())?;
        }
    }
    interp.exec_source_with_filename(
        LOCALE_PY_SOURCE,
        "<frozen locale>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("locale: exec namespace not a dict".into()))?;
    for &name in LOCALE_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("locale"));
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

pyrust_module! {
    constants {
        "CHAR_MAX" => Value::int(127),
        "LC_ALL" => Value::int(HOST_LC_ALL),
        "LC_COLLATE" => Value::int(HOST_LC_COLLATE),
        "LC_CTYPE" => Value::int(HOST_LC_CTYPE),
        "LC_MONETARY" => Value::int(HOST_LC_MONETARY),
        "LC_NUMERIC" => Value::int(HOST_LC_NUMERIC),
        "LC_TIME" => Value::int(HOST_LC_TIME),
    }
}
