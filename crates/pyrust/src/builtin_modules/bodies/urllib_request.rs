// `urllib.request` module — body for the `"urllib.request" as urllib_request @inject`
// entry in `pyrust_builtin_modules!` (issue #3223).
//
// Pure-Python extensible URL opening library built on `http.client`, `socket`,
// `urllib.parse`, `urllib.error`, `urllib.response`, `email`, and `mimetypes`,
// aligned with CPython 3.12's `Lib/urllib/request.py`.
//
// Reference: <https://docs.python.org/3/library/urllib.request.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `urllib.request` module.
const URLLIB_REQUEST_PY_SOURCE: &str = include_str!("urllib_request_py.py");

/// Public and compatibility names exported from `URLLIB_REQUEST_PY_SOURCE`.
const URLLIB_REQUEST_PY_EXPORTS: &[&str] = &[
    "__all__",
    "__version__",
    "Request",
    "OpenerDirector",
    "BaseHandler",
    "HTTPDefaultErrorHandler",
    "HTTPRedirectHandler",
    "ProxyHandler",
    "HTTPPasswordMgr",
    "HTTPPasswordMgrWithDefaultRealm",
    "HTTPPasswordMgrWithPriorAuth",
    "AbstractBasicAuthHandler",
    "HTTPBasicAuthHandler",
    "ProxyBasicAuthHandler",
    "AbstractDigestAuthHandler",
    "HTTPDigestAuthHandler",
    "ProxyDigestAuthHandler",
    "AbstractHTTPHandler",
    "HTTPHandler",
    "FileHandler",
    "DataHandler",
    "UnknownHandler",
    "HTTPErrorProcessor",
    "urlopen",
    "install_opener",
    "build_opener",
    "pathname2url",
    "url2pathname",
    "getproxies",
    "getproxies_environment",
    "proxy_bypass",
    "proxy_bypass_environment",
    "urlretrieve",
    "urlcleanup",
    "parse_http_list",
    "parse_keqv_list",
    "request_host",
    "localhost",
    "thishost",
    "noheaders",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(URLLIB_REQUEST_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("urllib.request: exec namespace not a dict".into()))?;
    for &name in URLLIB_REQUEST_PY_EXPORTS {
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
