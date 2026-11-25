// `http.client` module — body for the `"http.client" as http_client @inject`
// entry in `pyrust_builtin_modules!` (issue #3207).
//
// Pure-Python HTTP/1.1 client library built on `socket`, `http`, and
// `urllib.parse`: `HTTPConnection`, `HTTPResponse`, `HTTPMessage`,
// `parse_headers`, `responses`, status constants, and the `HTTPException`
// hierarchy.
//
// Reference: <https://docs.python.org/3/library/http.client.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `http.client` module.
const HTTP_CLIENT_PY_SOURCE: &str = include_str!("http_client_py.py");

/// Public and compatibility names exported from `HTTP_CLIENT_PY_SOURCE`.
const HTTP_CLIENT_PY_EXPORTS: &[&str] = &[
    "__all__",
    "HTTP_PORT",
    "HTTPS_PORT",
    "responses",
    "HTTPMessage",
    "parse_headers",
    "HTTPResponse",
    "HTTPConnection",
    "HTTPSConnection",
    "_create_https_context",
    "HTTPException",
    "NotConnected",
    "InvalidURL",
    "UnknownProtocol",
    "UnknownTransferEncoding",
    "UnimplementedFileMode",
    "IncompleteRead",
    "ImproperConnectionState",
    "CannotSendRequest",
    "CannotSendHeader",
    "ResponseNotReady",
    "BadStatusLine",
    "LineTooLong",
    "RemoteDisconnected",
    "error",
    "_MAXLINE",
    "_MAXHEADERS",
    // Backwards-compatible HTTPStatus constants mirrored onto http.client.
    "CONTINUE",
    "SWITCHING_PROTOCOLS",
    "PROCESSING",
    "EARLY_HINTS",
    "OK",
    "CREATED",
    "ACCEPTED",
    "NON_AUTHORITATIVE_INFORMATION",
    "NO_CONTENT",
    "RESET_CONTENT",
    "PARTIAL_CONTENT",
    "MULTI_STATUS",
    "ALREADY_REPORTED",
    "IM_USED",
    "MULTIPLE_CHOICES",
    "MOVED_PERMANENTLY",
    "FOUND",
    "SEE_OTHER",
    "NOT_MODIFIED",
    "USE_PROXY",
    "TEMPORARY_REDIRECT",
    "PERMANENT_REDIRECT",
    "BAD_REQUEST",
    "UNAUTHORIZED",
    "PAYMENT_REQUIRED",
    "FORBIDDEN",
    "NOT_FOUND",
    "METHOD_NOT_ALLOWED",
    "NOT_ACCEPTABLE",
    "PROXY_AUTHENTICATION_REQUIRED",
    "REQUEST_TIMEOUT",
    "CONFLICT",
    "GONE",
    "LENGTH_REQUIRED",
    "PRECONDITION_FAILED",
    "REQUEST_ENTITY_TOO_LARGE",
    "REQUEST_URI_TOO_LONG",
    "UNSUPPORTED_MEDIA_TYPE",
    "REQUESTED_RANGE_NOT_SATISFIABLE",
    "EXPECTATION_FAILED",
    "IM_A_TEAPOT",
    "MISDIRECTED_REQUEST",
    "UNPROCESSABLE_ENTITY",
    "LOCKED",
    "FAILED_DEPENDENCY",
    "TOO_EARLY",
    "UPGRADE_REQUIRED",
    "PRECONDITION_REQUIRED",
    "TOO_MANY_REQUESTS",
    "REQUEST_HEADER_FIELDS_TOO_LARGE",
    "UNAVAILABLE_FOR_LEGAL_REASONS",
    "INTERNAL_SERVER_ERROR",
    "NOT_IMPLEMENTED",
    "BAD_GATEWAY",
    "SERVICE_UNAVAILABLE",
    "GATEWAY_TIMEOUT",
    "HTTP_VERSION_NOT_SUPPORTED",
    "VARIANT_ALSO_NEGOTIATES",
    "INSUFFICIENT_STORAGE",
    "LOOP_DETECTED",
    "NOT_EXTENDED",
    "NETWORK_AUTHENTICATION_REQUIRED",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(HTTP_CLIENT_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("http.client: exec namespace not a dict".into()))?;
    for &name in HTTP_CLIENT_PY_EXPORTS {
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
