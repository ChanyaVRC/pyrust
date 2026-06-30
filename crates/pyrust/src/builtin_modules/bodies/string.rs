// `string` module — character-class constants plus the `Template`
// ($-substitution) and `Formatter` (PEP 3101) classes.
//
// Included into `pub mod string { … }` declared by the
// `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs`.
//
// The constants are supplied natively here (they are just frozen ASCII
// strings).  `capwords`, `Template`, and `Formatter` are defined in
// Python (`string_py.py`) and injected onto the module by the post-load
// hook in `env.rs::load_module`, mirroring the `collections` / `asyncio`
// approach.  Implementing them in Python avoids re-deriving CPython's
// `re`-based Template scanner and `_string`-based Formatter parser in
// Rust (pyrust ships neither `re` nor `_string`).
//
// Reference: <https://docs.python.org/3/library/string.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-level members of the module (`capwords`, `Template`,
/// `Formatter`) defined as ordinary Python source.
const STRING_PY_SOURCE: &str = include_str!("string_py.py");

/// Names from `STRING_PY_SOURCE` exported onto the `string` module.
const STRING_PY_EXPORTS: [&str; 3] = ["capwords", "Template", "Formatter"];

/// Execute `STRING_PY_SOURCE` once and copy its public names onto the
/// `string` module's attribute map.  Wired from `env.rs::load_module`'s
/// post-import hook (mirrors the `collections` / `asyncio` injection).
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(STRING_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| crate::error::PyError::Runtime("string: exec namespace not a dict".into()))?;
    for name in STRING_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        // ASCII character-class strings, byte-for-byte the values CPython
        // 3.12 exposes (verified via `python3.12 -c "import string; ..."`).
        "ascii_lowercase" => Value::string("abcdefghijklmnopqrstuvwxyz"),
        "ascii_uppercase" => Value::string("ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        "ascii_letters"   => Value::string("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"),
        "digits"          => Value::string("0123456789"),
        "hexdigits"       => Value::string("0123456789abcdefABCDEF"),
        "octdigits"       => Value::string("01234567"),
        "punctuation"     => Value::string("!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~"),
        "whitespace"      => Value::string(" \t\n\r\x0b\x0c"),
        "printable"       => Value::string(
            "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\
             !\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~ \t\n\r\x0b\x0c"
        ),
    }
}
