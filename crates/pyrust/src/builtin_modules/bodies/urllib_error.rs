// `urllib.error` module — 100% native Rust implementation of CPython 3.12's
// `Lib/urllib/error.py` (issues #3223, #3443).
//
// The exception hierarchy `URLError(OSError)`, `HTTPError(URLError,
// urllib.response.addinfourl)` and `ContentTooShortError(URLError)`.
// `HTTPError` derives from `urllib.response.addinfourl` exactly as in CPython:
// that second base (and the `io` / `urllib` names the CPython source gets from
// its `import` statements) is wired in by `prepare_module`, which runs before
// the module enters the import cache.
//
// Reference: <https://docs.python.org/3/library/urllib.error.html>

use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::Result;
use crate::interpreter::builtin_args::PyValueRef;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

const MODULE_DOC: &str = "Exception classes raised by urllib.\n\n\
The base exception class is URLError, which inherits from OSError.  It\n\
doesn't define any behavior of its own, but is the base class for all\n\
exceptions defined in this package.\n\n\
HTTPError is an exception class that is also a valid HTTP response\n\
instance.  It behaves this way because HTTP protocol errors are valid\n\
responses, with a status code, headers, and a body.  In some contexts,\n\
an application may want to handle an exception like a regular\n\
response.\n";

// `def`s of `Lib/urllib/error.py`: CPython reports arity errors for these in
// the interpreter's `initialize_locals` wording, not the macro's.
const SIG_URL_ERROR_INIT: Sig = Sig {
    qualname: "URLError.__init__",
    params: &["self", "reason", "filename"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_HTTP_ERROR_INIT: Sig = Sig {
    qualname: "HTTPError.__init__",
    params: &["self", "url", "code", "msg", "hdrs", "fp"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_CONTENT_TOO_SHORT_INIT: Sig = Sig {
    qualname: "ContentTooShortError.__init__",
    params: &["self", "message", "content"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

fn call_pos(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

/// `fmt % value` with CPython's `%` formatting semantics (so a tuple `value`
/// is spread over the conversions exactly like the CPython source does).
fn percent_format(interp: &mut Interpreter, fmt: &str, value: Value) -> Result<Value> {
    interp.eval_binary(Value::string(fmt), BinaryOp::Mod, value)
}

/// Wire what the CPython source gets from its imports and class statements:
/// `import io`, `import urllib.response`, the second base of `HTTPError`, and
/// the `__super_init = urllib.response.addinfourl.__init__` class attribute.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    if let Ok(io) = interp.load_module("io") {
        module_rc.borrow_mut().insert_attr("io".to_string(), io);
    }
    let Ok(response) = interp.load_module("urllib.response") else {
        return;
    };
    if let Ok(urllib) = interp.load_module("urllib") {
        module_rc
            .borrow_mut()
            .insert_attr("urllib".to_string(), urllib);
    }
    let http_error = module_rc.borrow().attrs.get("HTTPError").cloned();
    let Some(http_error) = http_error else {
        return;
    };
    let ValueKind::PyClass(http_rc) = http_error.kind() else {
        return;
    };
    if let Ok(addinfourl) = interp.get_attr(&response, "addinfourl")
        && let ValueKind::PyClass(addinfourl_rc) = addinfourl.kind()
    {
        http_rc.borrow_mut().extra_bases = vec![Rc::clone(addinfourl_rc)];
        addinfourl_rc
            .borrow()
            .subclasses
            .borrow_mut()
            .push(Rc::downgrade(http_rc));
        if let Ok(init) = interp.get_attr(&addinfourl, "__init__") {
            http_rc
                .borrow_mut()
                .attrs
                .insert("_HTTPError__super_init".to_string(), init);
        }
    }
    // `@property` objects bound in a class body know their name.
    for name in ["reason", "headers"] {
        let prop = http_rc.borrow().attrs.get(name).cloned();
        if let Some(prop) = prop
            && let Ok(set_name) = interp.get_attr(&prop, "__set_name__")
        {
            let _ = call_pos(interp, set_name, &[http_error.clone(), Value::string(name)]);
        }
    }
}

fn all_list() -> Value {
    Value::list(
        ["URLError", "HTTPError", "ContentTooShortError"]
            .into_iter()
            .map(Value::string)
            .collect(),
    )
}

/// CPython skips `OSError`'s own argument parsing when a subclass overrides
/// `__init__` (`oserror_use_init`), leaving `args == ()` and `errno`,
/// `strerror` and `filename2` unset.  The shared allocator here always parses
/// the constructor arguments, so undo that before the `__init__` body runs.
fn reset_os_error_state(interp: &mut Interpreter, self_val: &Value) -> Result<()> {
    interp.assign_attr(self_val.clone(), "args", Value::tuple(Vec::new()))?;
    for name in ["errno", "strerror", "filename2"] {
        interp.assign_attr(self_val.clone(), name, Value::none())?;
    }
    Ok(())
}

/// `URLError.__init__` body: `self.args = reason,`, `self.reason = reason`,
/// and `self.filename = filename` when given.
fn url_error_init(
    interp: &mut Interpreter,
    self_val: &Value,
    reason: &Value,
    filename: Option<&Value>,
) -> Result<()> {
    reset_os_error_state(interp, self_val)?;
    interp.assign_attr(self_val.clone(), "args", Value::tuple(vec![reason.clone()]))?;
    interp.assign_attr(self_val.clone(), "reason", reason.clone())?;
    if let Some(filename) = filename
        && !filename.is_none()
    {
        interp.assign_attr(self_val.clone(), "filename", filename.clone())?;
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
    }

    class URLError("OSError") {
        constants {
            "__doc__" => Value::none(),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_URL_ERROR_INIT, args)?;
            let this = b.take_or_none(0);
            let reason = b.take_or_none(1);
            let filename = b.take(2);
            url_error_init(_interp, &this, &reason, filename.as_ref())?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let reason = _interp.get_attr(self_val.0, "reason")?;
            percent_format(_interp, "<urlopen error %s>", reason)
        }
    }

    class HTTPError("URLError") {
        constants {
            "__doc__" => Value::string(
                "Raised when HTTP error occurs, but also acts like non-error return"
            ),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_HTTP_ERROR_INIT, args)?;
            let this = b.take_or_none(0);
            let url = b.take_or_none(1);
            let code = b.take_or_none(2);
            let msg = b.take_or_none(3);
            let hdrs = b.take_or_none(4);
            let fp = b.take_or_none(5);
            reset_os_error_state(_interp, &this)?;
            _interp.assign_attr(this.clone(), "code", code.clone())?;
            _interp.assign_attr(this.clone(), "msg", msg)?;
            _interp.assign_attr(this.clone(), "hdrs", hdrs.clone())?;
            _interp.assign_attr(this.clone(), "fp", fp.clone())?;
            _interp.assign_attr(this.clone(), "filename", url.clone())?;
            let fp_arg = if fp.is_none() {
                let io = _interp.load_module("io")?;
                let bytes_io = _interp.get_attr(&io, "BytesIO")?;
                call_pos(_interp, bytes_io, &[])?
            } else {
                fp
            };
            // `self.__super_init(fp, hdrs, url, code)`.  A native method kept
            // under another name in the class dict is dispatched by its own
            // name (`__init__`), which would re-enter this method, so call
            // `urllib.response.addinfourl.__init__` through its class.
            let response = _interp.load_module("urllib.response")?;
            let addinfourl = _interp.get_attr(&response, "addinfourl")?;
            let super_init = _interp.get_attr(&addinfourl, "__init__")?;
            call_pos(_interp, super_init, &[this, fp_arg, hdrs, url, code])?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let code = _interp.get_attr(self_val.0, "code")?;
            let msg = _interp.get_attr(self_val.0, "msg")?;
            percent_format(_interp, "HTTP Error %s: %s", Value::tuple(vec![code, msg]))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let code = _interp.get_attr(self_val.0, "code")?;
            let msg = _interp.get_attr(self_val.0, "msg")?;
            percent_format(_interp, "<HTTPError %s: %r>", Value::tuple(vec![code, msg]))
        }

        #[getter]
        fn reason(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "msg")
        }

        #[getter]
        fn headers(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "hdrs")
        }

        #[setter("headers")]
        fn set_headers(
            #[positional_only] self_val: PyValueRef<'_>,
            headers: PyValueRef<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "hdrs", headers.0.clone())?;
            Ok(Value::none())
        }
    }

    class ContentTooShortError("URLError") {
        constants {
            "__doc__" => Value::string(
                "Exception raised when downloaded size does not match content-length."
            ),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_CONTENT_TOO_SHORT_INIT, args)?;
            let this = b.take_or_none(0);
            let message = b.take_or_none(1);
            let content = b.take_or_none(2);
            url_error_init(_interp, &this, &message, None)?;
            _interp.assign_attr(this, "content", content)?;
            Ok(Value::none())
        }
    }
}
