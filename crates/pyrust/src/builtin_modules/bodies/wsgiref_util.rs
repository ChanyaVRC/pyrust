// `wsgiref.util` module — 100% native-Rust Python 3.12 implementation using
// PyRust's expanded macro toolkit (`pyrust_module!`, `class`).
//
// Provides `FileWrapper`, `guess_scheme`, `application_uri`, `request_uri`,
// `shift_path_info`, `setup_testing_defaults`, and `is_hop_by_hop`.
//
// Reference: <https://docs.python.org/3/library/wsgiref.html#module-wsgiref.util>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyInt, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyKey, Value};

const HOP_BY_HOP_HEADERS: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

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

fn quote_latin1(s: &str, safe: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let code = ch as u32;
        if code <= 0xFF {
            let b = code as u8;
            let unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~');
            if unreserved || (b < 128 && safe.as_bytes().contains(&b)) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            let mut buf = [0u8; 4];
            for &b in ch.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

fn environ_get_opt(interp: &mut Interpreter, environ: &Value, key: &str) -> Result<Option<Value>> {
    if let Some(d) = environ.as_dict() {
        return Ok(d.get(&PyKey::str_from(key)).cloned());
    }
    let get_fn = interp.get_attr(environ, "get")?;
    let val = call_pos(interp, get_fn, &[Value::string(key), Value::none()])?;
    if val.is_none() {
        Ok(None)
    } else {
        Ok(Some(val))
    }
}

fn environ_get_required(interp: &mut Interpreter, environ: &Value, key: &str) -> Result<String> {
    let v = if let Some(d) = environ.as_dict() {
        d.get(&PyKey::str_from(key))
            .cloned()
            .ok_or_else(|| PyError::named("KeyError", format!("{key:?}")))?
    } else {
        interp.eval_index(environ, Value::string(key))?
    };
    v.as_str()
        .map(str::to_string)
        .ok_or_else(|| PyError::named("TypeError", format!("environ[{key:?}] must be a str")))
}

fn environ_set(interp: &mut Interpreter, environ: &Value, key: &str, val: Value) -> Result<()> {
    if environ.as_dict().is_some() {
        environ.dict_insert(PyKey::str_from(key), val)?;
        return Ok(());
    }
    call_method(interp, environ, "__setitem__", &[Value::string(key), val])?;
    Ok(())
}

fn environ_setdefault(
    interp: &mut Interpreter,
    environ: &Value,
    key: &str,
    default: Value,
) -> Result<Value> {
    if let Some(existing) = environ_get_opt(interp, environ, key)? {
        return Ok(existing);
    }
    environ_set(interp, environ, key, default.clone())?;
    Ok(default)
}

fn guess_scheme_impl(interp: &mut Interpreter, environ: &Value) -> Result<&'static str> {
    if let Some(v) = environ_get_opt(interp, environ, "HTTPS")?
        && let Some(s) = v.as_str()
        && matches!(s, "yes" | "on" | "1")
    {
        return Ok("https");
    }
    Ok("http")
}

fn application_uri_impl(interp: &mut Interpreter, environ: &Value) -> Result<String> {
    let scheme = environ_get_required(interp, environ, "wsgi.url_scheme")?;
    let mut url = format!("{scheme}://");
    if let Some(http_host) = environ_get_opt(interp, environ, "HTTP_HOST")?
        && let Some(s) = http_host.as_str()
        && !s.is_empty()
    {
        url.push_str(s);
    } else {
        let server_name = environ_get_required(interp, environ, "SERVER_NAME")?;
        url.push_str(&server_name);
        let server_port = environ_get_required(interp, environ, "SERVER_PORT")?;
        if (scheme == "https" && server_port != "443") || (scheme != "https" && server_port != "80")
        {
            url.push(':');
            url.push_str(&server_port);
        }
    }
    let script_name = environ_get_opt(interp, environ, "SCRIPT_NAME")?
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let tail = if script_name.is_empty() {
        "/".to_string()
    } else {
        quote_latin1(&script_name, "/")
    };
    url.push_str(&tail);
    Ok(url)
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("FileWrapper"),
            Value::string("guess_scheme"),
            Value::string("application_uri"),
            Value::string("request_uri"),
            Value::string("shift_path_info"),
            Value::string("setup_testing_defaults"),
            Value::string("is_hop_by_hop"),
        ]),
    }

    class FileWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            filelike: PyValue,
            #[default(PyInt::from(8192i64))] blksize: PyInt,
        ) -> Result<Value> {
            let bs = blksize.expect_i64(FN_NAME, "blksize")?;
            _interp.assign_attr(self_val.0.clone(), "filelike", filelike.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "blksize", Value::int(bs))?;
            if let Ok(close_attr) = _interp.get_attr(&filelike.0, "close") {
                _interp.assign_attr(self_val.0.clone(), "close", close_attr)?;
            }
            Ok(Value::none())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let filelike = _interp.get_attr(self_val.0, "filelike")?;
            let blksize = _interp.get_attr(self_val.0, "blksize")?;
            let data = call_method(_interp, &filelike, "read", &[blksize])?;
            if _interp.truthy_value(&data)? {
                Ok(data)
            } else {
                Err(PyError::named("StopIteration", ""))
            }
        }
    }

    fn guess_scheme(environ: PyValue) -> Result<Value> {
        Ok(Value::string(guess_scheme_impl(_interp, &environ.0)?))
    }

    fn application_uri(environ: PyValue) -> Result<Value> {
        Ok(Value::string(application_uri_impl(_interp, &environ.0)?))
    }

    fn request_uri(
        environ: PyValue,
        #[default(true.into())] include_query: PyBool,
    ) -> Result<Value> {
        let mut url = application_uri_impl(_interp, &environ.0)?;
        let raw_path = environ_get_opt(_interp, &environ.0, "PATH_INFO")?
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let path_info = quote_latin1(&raw_path, "/;=,");
        let script_name = environ_get_opt(_interp, &environ.0, "SCRIPT_NAME")?
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        if script_name.is_empty() {
            if let Some(rest) = path_info.strip_prefix('/') {
                url.push_str(rest);
            } else {
                url.push_str(&path_info);
            }
        } else {
            url.push_str(&path_info);
        }
        if include_query.0
            && let Some(qs) = environ_get_opt(_interp, &environ.0, "QUERY_STRING")?
            && let Some(q) = qs.as_str()
            && !q.is_empty()
        {
            url.push('?');
            url.push_str(q);
        }
        Ok(Value::string(url))
    }

    fn shift_path_info(environ: PyValue) -> Result<Value> {
        let path_info = environ_get_opt(_interp, &environ.0, "PATH_INFO")?
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        if path_info.is_empty() {
            return Ok(Value::none());
        }
        let mut path_parts: Vec<&str> = path_info.split('/').collect();
        // Filter out empty or "." segments between the first and last element.
        if path_parts.len() > 2 {
            let first = path_parts[0];
            let last = *path_parts.last().unwrap();
            let mut filtered = vec![first];
            for &seg in &path_parts[1..path_parts.len() - 1] {
                if !seg.is_empty() && seg != "." {
                    filtered.push(seg);
                }
            }
            filtered.push(last);
            path_parts = filtered;
        }
        let name = if path_parts.len() > 1 {
            path_parts.remove(1)
        } else {
            return Ok(Value::none());
        };
        let mut script_name = environ_get_opt(_interp, &environ.0, "SCRIPT_NAME")?
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let trimmed = script_name.trim_end_matches('/');
        script_name = format!("{trimmed}/{name}");
        if script_name.ends_with("/.") {
            script_name.truncate(script_name.len() - 2);
        }
        let new_path_info = path_parts.join("/");
        environ_set(
            _interp,
            &environ.0,
            "SCRIPT_NAME",
            Value::string(script_name),
        )?;
        environ_set(
            _interp,
            &environ.0,
            "PATH_INFO",
            Value::string(new_path_info),
        )?;
        if name == "." {
            return Ok(Value::string(""));
        }
        Ok(Value::string(name))
    }

    fn setup_testing_defaults(environ: PyValue) -> Result<Value> {
        let server_name =
            environ_setdefault(_interp, &environ.0, "SERVER_NAME", Value::string("127.0.0.1"))?;
        environ_setdefault(
            _interp,
            &environ.0,
            "SERVER_PROTOCOL",
            Value::string("HTTP/1.0"),
        )?;
        environ_setdefault(_interp, &environ.0, "HTTP_HOST", server_name)?;
        environ_setdefault(_interp, &environ.0, "REQUEST_METHOD", Value::string("GET"))?;

        let has_sn = environ_get_opt(_interp, &environ.0, "SCRIPT_NAME")?.is_some();
        let has_pi = environ_get_opt(_interp, &environ.0, "PATH_INFO")?.is_some();
        if !has_sn && !has_pi {
            environ_setdefault(_interp, &environ.0, "SCRIPT_NAME", Value::string(""))?;
            environ_setdefault(_interp, &environ.0, "PATH_INFO", Value::string("/"))?;
        }
        environ_setdefault(
            _interp,
            &environ.0,
            "wsgi.version",
            Value::tuple(vec![Value::int(1), Value::int(0)]),
        )?;
        environ_setdefault(_interp, &environ.0, "wsgi.run_once", Value::int(0))?;
        environ_setdefault(_interp, &environ.0, "wsgi.multithread", Value::int(0))?;
        environ_setdefault(_interp, &environ.0, "wsgi.multiprocess", Value::int(0))?;

        let io_mod = _interp.load_module("io")?;
        if environ_get_opt(_interp, &environ.0, "wsgi.input")?.is_none() {
            let bytes_io = _interp.get_attr(&io_mod, "BytesIO")?;
            let bio = call_pos(_interp, bytes_io, &[])?;
            environ_set(_interp, &environ.0, "wsgi.input", bio)?;
        }
        if environ_get_opt(_interp, &environ.0, "wsgi.errors")?.is_none() {
            let string_io = _interp.get_attr(&io_mod, "StringIO")?;
            let sio = call_pos(_interp, string_io, &[])?;
            environ_set(_interp, &environ.0, "wsgi.errors", sio)?;
        }
        let guessed = guess_scheme_impl(_interp, &environ.0)?;
        let scheme_val = environ_setdefault(
            _interp,
            &environ.0,
            "wsgi.url_scheme",
            Value::string(guessed),
        )?;
        let default_port = if scheme_val.as_str() == Some("http") {
            "80"
        } else {
            "443"
        };
        environ_setdefault(
            _interp,
            &environ.0,
            "SERVER_PORT",
            Value::string(default_port),
        )?;
        Ok(Value::none())
    }

    fn is_hop_by_hop(header_name: PyStr<'_>) -> Result<Value> {
        let lower = header_name.0.to_ascii_lowercase();
        Ok(Value::bool_(HOP_BY_HOP_HEADERS.contains(&lower.as_str())))
    }
}
