// `wsgiref.validate` module — 100% native-Rust Python 3.12 PEP 3333 WSGI
// compliance validator using PyRust's expanded macro toolkit (`pyrust_module!`,
// `class`, `#[native_state]`, `#[varargs]`, `#[varkwargs]`).
//
// Provides `validator(application)` along with `InputWrapper`, `ErrorWrapper`,
// `WriteWrapper`, `IteratorWrapper`, and `PartialIteratorWrapper`.
//
// Reference: <https://docs.python.org/3/library/wsgiref.html#module-wsgiref.validate>

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyKey, Value, ValueKind};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_pos(interp, m, args)
}

fn val_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("wsgiref.validate")?;
    interp.get_attr(&m, name)
}

fn assert_cond(cond: bool, msg: impl Into<String>) -> Result<()> {
    if !cond {
        return Err(PyError::named("AssertionError", msg.into()));
    }
    Ok(())
}

fn check_string_type(val: &Value, title: &str) -> Result<String> {
    match val.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        _ => Err(PyError::named(
            "AssertionError",
            format!(
                "{title} must be of type str (not {} of type {})",
                val.as_str().unwrap_or("<object>"),
                value_type_name_str(val)
            ),
        )),
    }
}

fn check_bytes_type(val: &Value, title: &str) -> Result<()> {
    assert_cond(
        matches!(val.kind(), ValueKind::Bytes(_)),
        format!(
            "{title} must be of type bytes (got {})",
            value_type_name_str(val)
        ),
    )
}

fn is_valid_header_name(name: &str) -> bool {
    if name.is_empty() || name.ends_with('_') || name.ends_with('-') {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!'))
}

fn is_bad_header_value(val: &str) -> bool {
    val.bytes().any(|b| b < 32 || b == 127)
}

fn check_status(status: &Value) -> Result<()> {
    let s = check_string_type(status, "Status")?;
    assert_cond(
        s.len() >= 5,
        format!("Status must be at least 5 characters: {s:?}"),
    )?;
    let (code_str, reason) = s.split_once(' ').ok_or_else(|| {
        PyError::named(
            "AssertionError",
            format!("Status must contain a space: {s:?}"),
        )
    })?;
    assert_cond(
        code_str.len() == 3 && code_str.bytes().all(|b| b.is_ascii_digit()),
        format!("Status code must be 3 digits: {code_str:?}"),
    )?;
    let code: i32 = code_str.parse().unwrap_or(0);
    assert_cond(code >= 100, format!("Status code must be >= 100: {code}"))?;
    assert_cond(
        !reason.is_empty(),
        format!("Status message must not be empty: {s:?}"),
    )?;
    assert_cond(
        !is_bad_header_value(reason),
        format!("Bad characters in status message: {s:?}"),
    )?;
    Ok(())
}

fn check_headers(interp: &mut Interpreter, headers: &Value) -> Result<()> {
    assert_cond(
        matches!(headers.kind(), ValueKind::List(_)),
        format!(
            "Headers ({}) must be of type list",
            value_type_name_str(headers)
        ),
    )?;
    let util = interp.load_module("wsgiref.util")?;
    let is_hbh = interp.get_attr(&util, "is_hop_by_hop")?;
    let items = interp.collect_iterable(headers)?;
    for item in items {
        assert_cond(
            matches!(item.kind(), ValueKind::Tuple(_)),
            format!(
                "Individual headers ({}) must be of type tuple",
                value_type_name_str(&item)
            ),
        )?;
        let pair = interp.collect_iterable(&item)?;
        assert_cond(
            pair.len() == 2,
            format!("Header tuple must have length 2, got {}", pair.len()),
        )?;
        let name = check_string_type(&pair[0], "Header name")?;
        let val = check_string_type(&pair[1], "Header value")?;
        assert_cond(
            !name.eq_ignore_ascii_case("status"),
            "The Status header cannot be used; it conflicts with CGI script, and HTTP status is not given through headers",
        )?;
        assert_cond(
            !name.contains('\n') && !name.contains(':'),
            format!("Header names may not contain ':' or '\\n': {name:?}"),
        )?;
        assert_cond(
            is_valid_header_name(&name),
            format!("Bad header name: {name:?}"),
        )?;
        assert_cond(
            !is_bad_header_value(&val),
            format!("Bad header value: {val:?} (bad char in header {name:?})"),
        )?;
        let bad = call_pos(interp, is_hbh.clone(), &[Value::string(&name)])?;
        assert_cond(
            !interp.truthy_value(&bad)?,
            format!("Hop-by-hop headers are not allowed: {name:?}"),
        )?;
    }
    Ok(())
}

fn check_content_type(interp: &mut Interpreter, status: &str, headers: &Value) -> Result<()> {
    let code: i32 = status
        .split(' ')
        .next()
        .and_then(|c| c.parse().ok())
        .unwrap_or(200);
    let no_message_body = (100..200).contains(&code) || code == 204 || code == 304;
    let items = interp.collect_iterable(headers)?;
    let mut has_ct = false;
    for item in items {
        let pair = interp.collect_iterable(&item)?;
        if pair.len() == 2
            && let Some(k) = pair[0].as_str()
            && k.eq_ignore_ascii_case("content-type")
        {
            has_ct = true;
            if no_message_body {
                return Err(PyError::named(
                    "AssertionError",
                    format!(
                        "Content-Type header found in a {code} response, which must not return content."
                    ),
                ));
            }
        }
    }
    if !no_message_body && !has_ct {
        return Err(PyError::named(
            "AssertionError",
            "No Content-Type header found in headers",
        ));
    }
    Ok(())
}

fn check_environ(interp: &mut Interpreter, environ: &Value) -> Result<()> {
    assert_cond(
        matches!(environ.kind(), ValueKind::Dict(_)),
        format!(
            "Environment is not of the right type: {} (environment: dict required)",
            value_type_name_str(environ)
        ),
    )?;
    let dict = environ.as_dict().unwrap();
    for key in [
        "REQUEST_METHOD",
        "SERVER_NAME",
        "SERVER_PORT",
        "wsgi.version",
        "wsgi.input",
        "wsgi.errors",
        "wsgi.multithread",
        "wsgi.multiprocess",
        "wsgi.run_once",
    ] {
        assert_cond(
            dict.get(&PyKey::str_from(key)).is_some(),
            format!("Environment missing required key: {key:?}"),
        )?;
    }
    for key in ["HTTP_CONTENT_TYPE", "HTTP_CONTENT_LENGTH"] {
        assert_cond(
            dict.get(&PyKey::str_from(key)).is_none(),
            format!(
                "Environment should not have the key: {key} (use {} instead)",
                &key[5..]
            ),
        )?;
    }
    if dict.get(&PyKey::str_from("QUERY_STRING")).is_none() {
        // CPython warns rather than asserts; acceptable.
    }
    for (k, v) in dict.iter() {
        let PyKey::Str(sv) = k else {
            return Err(PyError::named(
                "AssertionError",
                "Environment contains non-string key",
            ));
        };
        let ks = sv.as_str().unwrap_or_default();
        if !ks.contains('.') {
            check_string_type(v, &format!("Environment variable {ks}"))?;
        }
    }
    let ver = dict.get(&PyKey::str_from("wsgi.version")).unwrap();
    assert_cond(
        matches!(ver.kind(), ValueKind::Tuple(_)),
        "wsgi.version should be a tuple",
    )?;
    let ver_items = interp.collect_iterable(ver)?;
    assert_cond(ver_items.len() == 2, "wsgi.version should be a 2-tuple")?;
    let scheme = dict
        .get(&PyKey::str_from("wsgi.url_scheme"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_cond(
        scheme == "http" || scheme == "https",
        format!("wsgi.url_scheme unknown: {scheme:?}"),
    )?;

    let req_method = dict
        .get(&PyKey::str_from("REQUEST_METHOD"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_cond(
        !req_method.is_empty(),
        "REQUEST_METHOD must be a non-empty string",
    )?;

    let sn = dict
        .get(&PyKey::str_from("SCRIPT_NAME"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let pi = dict
        .get(&PyKey::str_from("PATH_INFO"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !sn.is_empty() {
        assert_cond(
            sn.starts_with('/'),
            format!("SCRIPT_NAME doesn't start with '/': {sn:?}"),
        )?;
    }
    if !pi.is_empty() {
        assert_cond(
            pi.starts_with('/'),
            format!("PATH_INFO doesn't start with '/': {pi:?}"),
        )?;
    }
    if let Some(cl) = dict.get(&PyKey::str_from("CONTENT_LENGTH"))
        && let Some(cls) = cl.as_str()
        && !cls.is_empty()
    {
        assert_cond(
            cls.parse::<i64>().is_ok_and(|n| n >= 0),
            format!("Invalid CONTENT_LENGTH: {cls:?}"),
        )?;
    }
    if let Some(inp) = dict.get(&PyKey::str_from("wsgi.input")) {
        for attr in ["read", "readline", "readlines", "__iter__"] {
            assert_cond(
                interp.get_attr(inp, attr).is_ok(),
                format!("wsgi.input doesn't have the attribute {attr}"),
            )?;
        }
    }
    if let Some(err) = dict.get(&PyKey::str_from("wsgi.errors")) {
        for attr in ["flush", "write", "writelines"] {
            assert_cond(
                interp.get_attr(err, attr).is_ok(),
                format!("wsgi.errors doesn't have the attribute {attr}"),
            )?;
        }
    }
    Ok(())
}

struct ValidatorAppState {
    application: Value,
}

struct StartResponseState {
    orig_start_response: Value,
    start_response_started: Value, // list holding bools/statuses so IteratorWrapper sees it
}

struct WriteWrapperState {
    writer: Value,
}

struct IteratorWrapperState {
    original_iterator: Value,
    iterator: Value,
    closed: bool,
    check_start_response: Value,
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![Value::string("validator")]),
    }

    class InputWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            wsgi_input: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "input", wsgi_input.0)?;
            Ok(Value::none())
        }

        fn read(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            assert_cond(args.len() <= 1, "input.read() takes at most 1 argument")?;
            let inp = _interp.get_attr(self_val.0, "input")?;
            let v = call_method(_interp, &inp, "read", args)?;
            check_bytes_type(&v, "input.read() return value")?;
            Ok(v)
        }

        fn readline(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            assert_cond(args.len() <= 1, "input.readline() takes at most 1 argument")?;
            let inp = _interp.get_attr(self_val.0, "input")?;
            let v = call_method(_interp, &inp, "readline", args)?;
            check_bytes_type(&v, "input.readline() return value")?;
            Ok(v)
        }

        fn readlines(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            assert_cond(args.len() <= 1, "input.readlines() takes at most 1 argument")?;
            let inp = _interp.get_attr(self_val.0, "input")?;
            let lines = call_method(_interp, &inp, "readlines", args)?;
            assert_cond(
                matches!(lines.kind(), ValueKind::List(_)),
                "input.readlines() must return a list",
            )?;
            for line in _interp.collect_iterable(&lines)? {
                check_bytes_type(&line, "input.readlines() line")?;
            }
            Ok(lines)
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let line = call_method(_interp, self_val.0, "readline", &[])?;
            if !_interp.truthy_value(&line)? {
                return Err(PyError::named("StopIteration", ""));
            }
            Ok(line)
        }

        fn close(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "AssertionError",
                "input.close() must not be called",
            ))
        }
    }

    class ErrorWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            wsgi_errors: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "errors", wsgi_errors.0)?;
            Ok(Value::none())
        }

        fn write(#[positional_only] self_val: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            check_string_type(&s.0, "errors.write() argument")?;
            let errs = _interp.get_attr(self_val.0, "errors")?;
            call_method(_interp, &errs, "write", &[s.0])?;
            Ok(Value::none())
        }

        fn flush(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let errs = _interp.get_attr(self_val.0, "errors")?;
            call_method(_interp, &errs, "flush", &[])?;
            Ok(Value::none())
        }

        fn writelines(
            #[positional_only] self_val: PyValueRef<'_>,
            seq: PyValue,
        ) -> Result<Value> {
            for line in _interp.collect_iterable(&seq.0)? {
                call_method(_interp, self_val.0, "write", &[line])?;
            }
            Ok(Value::none())
        }

        fn close(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "AssertionError",
                "errors.close() must not be called",
            ))
        }
    }

    #[native_state(WriteWrapperState)]
    class WriteWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            wsgi_writer: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                WriteWrapperState {
                    writer: wsgi_writer.0.clone(),
                },
            )?;
            _interp.assign_attr(self_val.0.clone(), "writer", wsgi_writer.0)?;
            Ok(Value::none())
        }

        fn __call__(#[positional_only] self_val: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            check_bytes_type(&s.0, "write() argument")?;
            let writer = with_state(self_val.0, |st| Ok(st.writer.clone()))?;
            call_pos(_interp, writer, &[s.0])
        }
    }

    #[native_state(StartResponseState)]
    class _StartResponseWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            orig: PyValue,
            started: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                StartResponseState {
                    orig_start_response: orig.0,
                    start_response_started: started.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            assert_cond(
                kw.is_empty(),
                "start_response() does not accept keyword arguments",
            )?;
            assert_cond(
                args.len() == 2 || args.len() == 3,
                format!("Two or three arguments expected; got {} arguments", args.len()),
            )?;
            let status = &args[0];
            let headers = &args[1];
            let exc_info = args.get(2).filter(|v| !v.is_none());

            let (orig, started) = with_state(self_val.0, |st| {
                Ok((
                    st.orig_start_response.clone(),
                    st.start_response_started.clone(),
                ))
            })?;
            let already_started = _interp.truthy_value(&started)?;
            if already_started && exc_info.is_none() {
                return Err(PyError::named(
                    "AssertionError",
                    "start_response() called a second time without exc_info",
                ));
            }
            if let Some(ei) = exc_info {
                assert_cond(
                    matches!(ei.kind(), ValueKind::Tuple(_)),
                    "exc_info must be a tuple",
                )?;
                let parts = _interp.collect_iterable(ei)?;
                assert_cond(parts.len() == 3, "exc_info must be a 3-tuple")?;
            }
            check_status(status)?;
            check_headers(_interp, headers)?;
            check_content_type(_interp, status.as_str().unwrap_or(""), headers)?;
            call_method(_interp, &started, "append", &[Value::bool_(true)])?;

            let writer = call_pos(_interp, orig, args)?;
            let ww_cls = val_attr(_interp, "WriteWrapper")?;
            call_pos(_interp, ww_cls, &[writer])
        }
    }

    #[native_state(IteratorWrapperState)]
    class IteratorWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            wsgi_iterator: PyValue,
            check_start_response: PyValue,
        ) -> Result<Value> {
            let iter_obj = call_pos(
                _interp,
                Value::builtin_function("iter"),
                std::slice::from_ref(&wsgi_iterator.0),
            )?;
            set_state(
                self_val.0,
                IteratorWrapperState {
                    original_iterator: wsgi_iterator.0.clone(),
                    iterator: iter_obj,
                    closed: false,
                    check_start_response: check_start_response.0,
                },
            )?;
            _interp.assign_attr(self_val.0.clone(), "original_iterator", wsgi_iterator.0)?;
            Ok(Value::none())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (closed, iter_obj, started) = with_state(self_val.0, |st| {
                Ok((
                    st.closed,
                    st.iterator.clone(),
                    st.check_start_response.clone(),
                ))
            })?;
            assert_cond(!closed, "Iterator read after closed")?;
            let v = call_pos(_interp, Value::builtin_function("next"), &[iter_obj])?;
            check_bytes_type(&v, "Iterator yielded non-bytes")?;
            if !_interp.truthy_value(&started)? {
                return Err(PyError::named(
                    "AssertionError",
                    "The application returns and we started iterating over its body, but start_response has not yet been called",
                ));
            }
            Ok(v)
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let orig = with_state_mut(self_val.0, |st| {
                st.closed = true;
                Ok(st.original_iterator.clone())
            })?;
            if let Ok(close_fn) = _interp.get_attr(&orig, "close") {
                call_pos(_interp, close_fn, &[])?;
            }
            Ok(Value::none())
        }
    }

    class PartialIteratorWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            wsgi_iterator: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "iterator", wsgi_iterator.0)?;
            Ok(Value::none())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let it = _interp.get_attr(self_val.0, "iterator")?;
            let iw = val_attr(_interp, "IteratorWrapper")?;
            call_pos(_interp, iw, &[it, Value::list(vec![Value::bool_(true)])])
        }
    }

    #[native_state(ValidatorAppState)]
    class _LintApp {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            application: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                ValidatorAppState {
                    application: application.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            assert_cond(kw.is_empty(), "WSGI application takes no keyword arguments")?;
            assert_cond(
                args.len() == 2,
                format!("Two arguments required; got {}", args.len()),
            )?;
            let environ = &args[0];
            let start_response = &args[1];
            check_environ(_interp, environ)?;

            // Wrap wsgi.input and wsgi.errors in validating wrappers.
            let inp = environ
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from("wsgi.input")).cloned())
                .unwrap();
            let err = environ
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from("wsgi.errors")).cloned())
                .unwrap();
            let iw_cls = val_attr(_interp, "InputWrapper")?;
            let ew_cls = val_attr(_interp, "ErrorWrapper")?;
            let wrapped_inp = call_pos(_interp, iw_cls, &[inp])?;
            let wrapped_err = call_pos(_interp, ew_cls, &[err])?;
            environ.dict_insert(PyKey::str_from("wsgi.input"), wrapped_inp)?;
            environ.dict_insert(PyKey::str_from("wsgi.errors"), wrapped_err)?;

            let started = Value::list(Vec::new());
            let sr_cls = val_attr(_interp, "_StartResponseWrapper")?;
            let wrapped_sr = call_pos(_interp, sr_cls, &[start_response.clone(), started.clone()])?;

            let app = with_state(self_val.0, |st| Ok(st.application.clone()))?;
            let iterator = call_pos(_interp, app, &[environ.clone(), wrapped_sr])?;
            assert_cond(
                !iterator.is_none()
                    && !matches!(iterator.kind(), ValueKind::Str(_) | ValueKind::Bytes(_)),
                "The application must return an iterator, if only an empty list",
            )?;
            let iter_cls = val_attr(_interp, "IteratorWrapper")?;
            call_pos(_interp, iter_cls, &[iterator, started])
        }
    }

    fn validator(application: PyValue) -> Result<Value> {
        let lint_cls = val_attr(_interp, "_LintApp")?;
        call_pos(_interp, lint_cls, &[application.0])
    }
}
