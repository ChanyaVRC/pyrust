// `urllib.parse` module — body for the `"urllib.parse" as urllib_parse @inject`
// entry in `pyrust_builtin_modules!` (issue #3149).
//
// CPython 3.12's `urllib.parse` is a pure-Python module. Shipping it as
// `urllib_parse_py.py` keeps URL parsing (`urlparse`, `urlunparse`, `urlsplit`,
// `urlunsplit`, `urljoin`, `urldefrag`), structured result types (`ParseResult`,
// `SplitResult`, `DefragResult` and `*Bytes` counterparts), percent-quoting
// (`quote`, `quote_plus`, `quote_from_bytes`, `unquote`, `unquote_plus`,
// `unquote_to_bytes`), and query helpers (`urlencode`, `parse_qs`, `parse_qsl`)
// aligned with CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/urllib.parse.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `urllib.parse` module.
const URLLIB_PARSE_PY_SOURCE: &str = include_str!("urllib_parse_py.py");

/// Public and compatibility names exported from `URLLIB_PARSE_PY_SOURCE`.
const URLLIB_PARSE_PY_EXPORTS: [&str; 43] = [
    "__all__",
    "uses_relative",
    "uses_netloc",
    "uses_params",
    "non_hierarchical",
    "uses_query",
    "uses_fragment",
    "scheme_chars",
    "clear_cache",
    "ResultBase",
    "DefragResult",
    "SplitResult",
    "ParseResult",
    "DefragResultBytes",
    "SplitResultBytes",
    "ParseResultBytes",
    "urlparse",
    "urlsplit",
    "urlunparse",
    "urlunsplit",
    "urljoin",
    "urldefrag",
    "unquote_to_bytes",
    "unquote",
    "unquote_plus",
    "parse_qs",
    "parse_qsl",
    "quote",
    "quote_plus",
    "quote_from_bytes",
    "urlencode",
    "unwrap",
    "to_bytes",
    "splittype",
    "splithost",
    "splituser",
    "splitpasswd",
    "splitport",
    "splitnport",
    "splitquery",
    "splittag",
    "splitattr",
    "splitvalue",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(URLLIB_PARSE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("urllib.parse: exec namespace not a dict".into()))?;
    for name in URLLIB_PARSE_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}
