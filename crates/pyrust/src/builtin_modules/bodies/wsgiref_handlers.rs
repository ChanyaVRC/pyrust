// `wsgiref.handlers` module — 100% native-Rust Python 3.12 implementation
// using PyRust's expanded macro toolkit (`pyrust_module!`, `class`).
//
// Provides `format_date_time`, `read_environ`, `BaseHandler`, `SimpleHandler`,
// `BaseCGIHandler`, `CGIHandler`, and `IISCGIHandler`.
//
// Reference: <https://docs.python.org/3/library/wsgiref.html#module-wsgiref.handlers>

use std::time::{SystemTime, UNIX_EPOCH};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyBool, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyDict, PyKey, Value, ValueKind};

const WEEKDAYNAME: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHNAME: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn format_http_date_secs(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400) as u32;
    let (y, m, d) = civil_from_days(days);
    let wd = (days + 3).rem_euclid(7) as usize;
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    let ss = secs % 60;
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        WEEKDAYNAME[wd],
        d,
        MONTHNAME[(m - 1) as usize],
        y,
        hh,
        mm,
        ss
    )
}

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

fn get_attr_truthy(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    let v = interp.get_attr(obj, name)?;
    interp.truthy_value(&v)
}

fn value_as_f64(val: &Value) -> Option<f64> {
    match val.kind() {
        ValueKind::Float(f) => Some(f),
        ValueKind::Int(i) => Some(i as f64),
        _ => None,
    }
}

fn read_environ_dict() -> Value {
    let mut dict = PyDict::default();
    for (k, v) in std::env::vars() {
        dict.insert(PyKey::str_from(&k), Value::string(v));
    }
    Value::dict(dict)
}

fn encode_iso_8859_1(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len());
    for ch in s.chars() {
        let code = ch as u32;
        if code > 0xFF {
            return Err(PyError::named(
                "UnicodeEncodeError",
                "'latin-1' codec can't encode character in WSGI header/status",
            ));
        }
        out.push(code as u8);
    }
    Ok(out)
}

fn handlers_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("wsgiref.handlers")?;
    interp.get_attr(&m, name)
}

fn default_headers_class(interp: &mut Interpreter, self_val: &Value) -> Result<Value> {
    if let Ok(hc) = interp.get_attr(self_val, "headers_class")
        && !hc.is_none()
    {
        return Ok(hc);
    }
    let m = interp.load_module("wsgiref.headers")?;
    interp.get_attr(&m, "Headers")
}

fn default_file_wrapper_class(interp: &mut Interpreter, self_val: &Value) -> Result<Value> {
    if let Ok(fw) = interp.get_attr(self_val, "wsgi_file_wrapper")
        && !fw.is_none()
    {
        return Ok(fw);
    }
    let m = interp.load_module("wsgiref.util")?;
    interp.get_attr(&m, "FileWrapper")
}

fn init_base_handler_attrs(interp: &mut Interpreter, self_val: &Value) -> Result<()> {
    let pairs: [(&str, Value); 5] = [
        ("result", Value::none()),
        ("headers", Value::none()),
        ("status", Value::none()),
        ("environ", Value::none()),
        ("headers_sent", Value::bool_(false)),
    ];
    for (k, v) in pairs {
        if interp.get_attr(self_val, k).is_err() {
            interp.assign_attr(self_val.clone(), k, v)?;
        }
    }
    if interp.get_attr(self_val, "bytes_sent").is_err() {
        interp.assign_attr(self_val.clone(), "bytes_sent", Value::int(0))?;
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("BaseHandler"),
            Value::string("SimpleHandler"),
            Value::string("BaseCGIHandler"),
            Value::string("CGIHandler"),
            Value::string("IISCGIHandler"),
            Value::string("read_environ"),
        ]),
    }

    fn format_date_time(timestamp: PyValue) -> Result<Value> {
        let secs = if timestamp.0.is_none() {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        } else if let Some(i) = timestamp.0.as_int() {
            i
        } else if let Some(f) = value_as_f64(&timestamp.0) {
            f.floor() as i64
        } else {
            return Err(PyError::named(
                "TypeError",
                "timestamp must be a real number or None",
            ));
        };
        Ok(Value::string(format_http_date_secs(secs)))
    }

    fn read_environ() -> Result<Value> {
        Ok(read_environ_dict())
    }

    class BaseHandler {
        constants {
            "wsgi_version" => Value::tuple(vec![Value::int(1), Value::int(0)]),
            "wsgi_multithread" => Value::bool_(true),
            "wsgi_multiprocess" => Value::bool_(true),
            "wsgi_run_once" => Value::bool_(false),
            "origin_server" => Value::bool_(true),
            "http_version" => Value::string("1.0"),
            "server_software" => Value::none(),
            "os_environ" => read_environ_dict(),
            "wsgi_file_wrapper" => Value::none(),
            "headers_class" => Value::none(),
            "traceback_limit" => Value::none(),
            "error_status" => Value::string("500 Internal Server Error"),
            "error_headers" => Value::list(vec![Value::tuple(vec![
                Value::string("Content-Type"),
                Value::string("text/plain"),
            ])]),
            "error_body" => Value::bytes(b"A server error occurred.  Please contact the administrator.".to_vec()),
            "status" => Value::none(),
            "result" => Value::none(),
            "headers_sent" => Value::bool_(false),
            "headers" => Value::none(),
            "bytes_sent" => Value::int(0),
            "environ" => Value::none(),
        }

        fn run(
            #[positional_only] self_val: PyValueRef<'_>,
            application: PyValue,
        ) -> Result<Value> {
            init_base_handler_attrs(_interp, self_val.0)?;
            let res: Result<()> = (|| {
                call_method(_interp, self_val.0, "setup_environ", &[])?;
                let environ = _interp.get_attr(self_val.0, "environ")?;
                let start_response = _interp.get_attr(self_val.0, "start_response")?;
                let result = call_pos(_interp, application.0, &[environ, start_response])?;
                _interp.assign_attr(self_val.0.clone(), "result", result)?;
                call_method(_interp, self_val.0, "finish_response", &[])?;
                Ok(())
            })();
            if let Err(err) = res {
                let exc = _interp.materialize_pyerror(err.clone())?;
                let sys_mod = _interp.load_module("sys")?;
                let exc_cls = crate::interpreter::value_class(&exc);
                let _ = _interp.assign_attr(
                    self_val.0.clone(),
                    "_last_exc_info",
                    Value::tuple(vec![exc_cls, exc, Value::none()]),
                );
                let _ = sys_mod;
                let handle_res = call_method(_interp, self_val.0, "handle_error", &[]);
                if handle_res.is_err() {
                    let _ = call_method(_interp, self_val.0, "close", &[]);
                    return Err(err);
                }
            }
            Ok(Value::none())
        }

        fn setup_environ(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let os_env = _interp.get_attr(self_val.0, "os_environ")?;
            let mut env = PyDict::default();
            if let Some(d) = os_env.as_dict() {
                for (k, v) in d.iter() {
                    env.insert(k.clone(), v.clone());
                }
            }
            let env_val = Value::dict(env);
            _interp.assign_attr(self_val.0.clone(), "environ", env_val.clone())?;
            call_method(_interp, self_val.0, "add_cgi_vars", &[])?;
            let env_val = _interp.get_attr(self_val.0, "environ")?;
            let stdin = call_method(_interp, self_val.0, "get_stdin", &[])?;
            let stderr = call_method(_interp, self_val.0, "get_stderr", &[])?;
            let wsgi_ver = _interp.get_attr(self_val.0, "wsgi_version")?;
            let multithread = _interp.get_attr(self_val.0, "wsgi_multithread")?;
            let multiprocess = _interp.get_attr(self_val.0, "wsgi_multiprocess")?;
            let run_once = _interp.get_attr(self_val.0, "wsgi_run_once")?;
            let scheme = call_method(_interp, self_val.0, "get_scheme", &[])?;

            env_val.dict_insert(PyKey::str_from("wsgi.input"), stdin)?;
            env_val.dict_insert(PyKey::str_from("wsgi.errors"), stderr)?;
            env_val.dict_insert(PyKey::str_from("wsgi.version"), wsgi_ver)?;
            env_val.dict_insert(PyKey::str_from("wsgi.run_once"), run_once)?;
            env_val.dict_insert(PyKey::str_from("wsgi.url_scheme"), scheme)?;
            env_val.dict_insert(PyKey::str_from("wsgi.multithread"), multithread)?;
            env_val.dict_insert(PyKey::str_from("wsgi.multiprocess"), multiprocess)?;
            let fw = default_file_wrapper_class(_interp, self_val.0)?;
            if !fw.is_none() {
                env_val.dict_insert(PyKey::str_from("wsgi.file_wrapper"), fw)?;
            }
            if get_attr_truthy(_interp, self_val.0, "origin_server")? {
                let sw = _interp.get_attr(self_val.0, "server_software")?;
                if _interp.truthy_value(&sw)?
                    && env_val
                        .as_dict()
                        .is_some_and(|d| d.get(&PyKey::str_from("SERVER_SOFTWARE")).is_none())
                {
                    env_val.dict_insert(PyKey::str_from("SERVER_SOFTWARE"), sw)?;
                }
            }
            Ok(Value::none())
        }

        fn finish_response(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let body_res: Result<()> = (|| {
                let is_file = call_method(_interp, self_val.0, "result_is_file", &[])?;
                let sent_file = if _interp.truthy_value(&is_file)? {
                    let sf = call_method(_interp, self_val.0, "sendfile", &[])?;
                    _interp.truthy_value(&sf)?
                } else {
                    false
                };
                if !sent_file {
                    let result = _interp.get_attr(self_val.0, "result")?;
                    let iter = call_pos(_interp, Value::builtin_function("iter"), &[result])?;
                    loop {
                        match call_pos(
                            _interp,
                            Value::builtin_function("next"),
                            std::slice::from_ref(&iter),
                        ) {
                            Ok(data) => {
                                call_method(_interp, self_val.0, "write", &[data])?;
                            }
                            Err(e) if e.class_name_is("StopIteration") => break,
                            Err(e) => return Err(e),
                        }
                    }
                    call_method(_interp, self_val.0, "finish_content", &[])?;
                }
                Ok(())
            })();
            let close_res = call_method(_interp, self_val.0, "close", &[]);
            body_res?;
            close_res?;
            Ok(Value::none())
        }

        fn get_scheme(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let environ = _interp.get_attr(self_val.0, "environ")?;
            let util = _interp.load_module("wsgiref.util")?;
            let gs = _interp.get_attr(&util, "guess_scheme")?;
            call_pos(_interp, gs, &[environ])
        }

        fn set_content_length(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let result = _interp.get_attr(self_val.0, "result")?;
            if let Ok(len_val) = call_pos(_interp, Value::builtin_function("len"), &[result])
                && len_val.as_int() == Some(1)
            {
                let bytes_sent = _interp
                    .get_attr(self_val.0, "bytes_sent")?
                    .as_int()
                    .unwrap_or(0);
                let headers = _interp.get_attr(self_val.0, "headers")?;
                call_method(
                    _interp,
                    &headers,
                    "__setitem__",
                    &[
                        Value::string("Content-Length"),
                        Value::string(bytes_sent.to_string()),
                    ],
                )?;
            }
            Ok(Value::none())
        }

        fn cleanup_headers(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let headers = _interp.get_attr(self_val.0, "headers")?;
            let has_cl = call_method(
                _interp,
                &headers,
                "__contains__",
                &[Value::string("Content-Length")],
            )?;
            if !_interp.truthy_value(&has_cl)? {
                call_method(_interp, self_val.0, "set_content_length", &[])?;
            }
            Ok(Value::none())
        }

        fn start_response(
            #[positional_only] self_val: PyValueRef<'_>,
            status: PyValue,
            headers: PyValue,
            #[default(OptionalArg::Omitted)] exc_info: OptValueRef<'_>,
        ) -> Result<Value> {
            if let Some(ei) = exc_info.as_value()
                && !ei.is_none()
            {
                let sent = get_attr_truthy(_interp, self_val.0, "headers_sent")?;
                if sent {
                    let parts = _interp.collect_iterable(ei)?;
                    if parts.len() >= 2 && !parts[1].is_none() {
                        return Err(PyError::raised(parts[1].clone()));
                    }
                    return Err(PyError::named("RuntimeError", "headers already sent"));
                }
            } else {
                let existing = _interp.get_attr(self_val.0, "headers")?;
                if !existing.is_none() {
                    return Err(PyError::named("AssertionError", "Headers already set!"));
                }
            }
            let status_str = status.0.as_str().ok_or_else(|| {
                PyError::named("AssertionError", "Status must be a str")
            })?;
            if status_str.len() < 4 || !status_str.as_bytes()[..3].iter().all(u8::is_ascii_digit)
                || status_str.as_bytes()[3] != b' '
            {
                return Err(PyError::named(
                    "AssertionError",
                    "Status must be at least 4 characters (e.g. '200 OK')",
                ));
            }
            let hdr_items = _interp.collect_iterable(&headers.0)?;
            let util = _interp.load_module("wsgiref.util")?;
            let is_hbh = _interp.get_attr(&util, "is_hop_by_hop")?;
            for item in &hdr_items {
                let pair = _interp.collect_iterable(item)?;
                if pair.len() != 2 {
                    return Err(PyError::named(
                        "AssertionError",
                        "Headers must be (name, value) tuples",
                    ));
                }
                let Some(k) = pair[0].as_str() else {
                    return Err(PyError::named("AssertionError", "Header names must be str"));
                };
                if pair[1].as_str().is_none() {
                    return Err(PyError::named("AssertionError", "Header values must be str"));
                }
                let bad = call_pos(_interp, is_hbh.clone(), &[Value::string(k)])?;
                if _interp.truthy_value(&bad)? {
                    return Err(PyError::named(
                        "AssertionError",
                        format!("Hop-by-hop header, '{k}', not allowed"),
                    ));
                }
            }
            _interp.assign_attr(self_val.0.clone(), "status", status.0)?;
            let hcls = default_headers_class(_interp, self_val.0)?;
            let hdr_obj = call_pos(_interp, hcls, &[headers.0])?;
            _interp.assign_attr(self_val.0.clone(), "headers", hdr_obj)?;
            _interp.get_attr(self_val.0, "write")
        }

        fn send_preamble(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let origin = get_attr_truthy(_interp, self_val.0, "origin_server")?;
            let status = _interp
                .get_attr(self_val.0, "status")?
                .as_str()
                .unwrap_or("200 OK")
                .to_string();
            if origin {
                let modern = call_method(_interp, self_val.0, "client_is_modern", &[])?;
                if _interp.truthy_value(&modern)? {
                    let http_ver = _interp
                        .get_attr(self_val.0, "http_version")?
                        .as_str()
                        .unwrap_or("1.0")
                        .to_string();
                    let line = format!("HTTP/{http_ver} {status}\r\n");
                    call_method(
                        _interp,
                        self_val.0,
                        "_write",
                        &[Value::bytes(encode_iso_8859_1(&line)?)],
                    )?;
                    let headers = _interp.get_attr(self_val.0, "headers")?;
                    let has_date = call_method(
                        _interp,
                        &headers,
                        "__contains__",
                        &[Value::string("Date")],
                    )?;
                    if !_interp.truthy_value(&has_date)? {
                        let now_secs = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        let date_line = format!("Date: {}\r\n", format_http_date_secs(now_secs));
                        call_method(
                            _interp,
                            self_val.0,
                            "_write",
                            &[Value::bytes(date_line.into_bytes())],
                        )?;
                    }
                    let sw = _interp.get_attr(self_val.0, "server_software")?;
                    if let Some(sw_s) = sw.as_str()
                        && !sw_s.is_empty()
                    {
                        let has_srv = call_method(
                            _interp,
                            &headers,
                            "__contains__",
                            &[Value::string("Server")],
                        )?;
                        if !_interp.truthy_value(&has_srv)? {
                            let srv_line = format!("Server: {sw_s}\r\n");
                            call_method(
                                _interp,
                                self_val.0,
                                "_write",
                                &[Value::bytes(encode_iso_8859_1(&srv_line)?)],
                            )?;
                        }
                    }
                }
            } else {
                let line = format!("Status: {status}\r\n");
                call_method(
                    _interp,
                    self_val.0,
                    "_write",
                    &[Value::bytes(encode_iso_8859_1(&line)?)],
                )?;
            }
            Ok(Value::none())
        }

        fn write(#[positional_only] self_val: PyValueRef<'_>, data: PyValue) -> Result<Value> {
            let data_len = match data.0.kind() {
                ValueKind::Bytes(b) => b.len(),
                _ => {
                    return Err(PyError::named(
                        "AssertionError",
                        format!(
                            "write() argument must be a bytes instance (got {})",
                            value_type_name_str(&data.0)
                        ),
                    ));
                }
            };
            if _interp.get_attr(self_val.0, "status")?.is_none() {
                return Err(PyError::named(
                    "AssertionError",
                    "write() before start_response()",
                ));
            }
            if !get_attr_truthy(_interp, self_val.0, "headers_sent")? {
                _interp.assign_attr(self_val.0.clone(), "bytes_sent", Value::int(data_len as i64))?;
                call_method(_interp, self_val.0, "send_headers", &[])?;
            } else {
                let prev = _interp
                    .get_attr(self_val.0, "bytes_sent")?
                    .as_int()
                    .unwrap_or(0);
                _interp.assign_attr(
                    self_val.0.clone(),
                    "bytes_sent",
                    Value::int(prev + data_len as i64),
                )?;
            }
            call_method(_interp, self_val.0, "_write", &[data.0])?;
            call_method(_interp, self_val.0, "_flush", &[])?;
            Ok(Value::none())
        }

        fn sendfile(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(false))
        }

        fn finish_content(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            if !get_attr_truthy(_interp, self_val.0, "headers_sent")? {
                let headers = _interp.get_attr(self_val.0, "headers")?;
                call_method(
                    _interp,
                    &headers,
                    "setdefault",
                    &[Value::string("Content-Length"), Value::string("0")],
                )?;
                call_method(_interp, self_val.0, "send_headers", &[])?;
            }
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let result = _interp.get_attr(self_val.0, "result")?;
            let close_res = if let Ok(close_fn) = _interp.get_attr(&result, "close") {
                call_pos(_interp, close_fn, &[])
            } else {
                Ok(Value::none())
            };
            _interp.assign_attr(self_val.0.clone(), "result", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "headers", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "status", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "environ", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "bytes_sent", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "headers_sent", Value::bool_(false))?;
            close_res?;
            Ok(Value::none())
        }

        fn send_headers(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "cleanup_headers", &[])?;
            _interp.assign_attr(self_val.0.clone(), "headers_sent", Value::bool_(true))?;
            let origin = get_attr_truthy(_interp, self_val.0, "origin_server")?;
            let modern = if origin {
                let m = call_method(_interp, self_val.0, "client_is_modern", &[])?;
                _interp.truthy_value(&m)?
            } else {
                true
            };
            if !origin || modern {
                call_method(_interp, self_val.0, "send_preamble", &[])?;
                let headers = _interp.get_attr(self_val.0, "headers")?;
                let hdr_bytes = call_pos(_interp, Value::builtin_function("bytes"), &[headers])?;
                call_method(_interp, self_val.0, "_write", &[hdr_bytes])?;
            }
            Ok(Value::none())
        }

        fn result_is_file(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let wrapper = default_file_wrapper_class(_interp, self_val.0)?;
            if wrapper.is_none() {
                return Ok(Value::bool_(false));
            }
            let result = _interp.get_attr(self_val.0, "result")?;
            call_pos(
                _interp,
                Value::builtin_function("isinstance"),
                &[result, wrapper],
            )
        }

        fn client_is_modern(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let env = _interp.get_attr(self_val.0, "environ")?;
            let proto = env
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from("SERVER_PROTOCOL")).cloned())
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            Ok(Value::bool_(!proto.eq_ignore_ascii_case("HTTP/0.9")))
        }

        fn log_exception(
            #[positional_only] self_val: PyValueRef<'_>,
            exc_info: PyValue,
        ) -> Result<Value> {
            let stderr = call_method(_interp, self_val.0, "get_stderr", &[])?;
            let parts = _interp.collect_iterable(&exc_info.0)?;
            if parts.len() >= 2 {
                let s = call_pos(
                    _interp,
                    Value::builtin_function("repr"),
                    std::slice::from_ref(&parts[1]),
                )?;
                let msg = format!("Traceback (most recent call last):\n{}\n", s.as_str().unwrap_or_default());
                let _ = call_method(_interp, &stderr, "write", &[Value::string(msg)]);
                let _ = call_method(_interp, &stderr, "flush", &[]);
            }
            Ok(Value::none())
        }

        fn handle_error(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ei = _interp
                .get_attr(self_val.0, "_last_exc_info")
                .unwrap_or_else(|_| Value::tuple(vec![Value::none(), Value::none(), Value::none()]));
            call_method(_interp, self_val.0, "log_exception", &[ei])?;
            if !get_attr_truthy(_interp, self_val.0, "headers_sent")? {
                let environ = _interp.get_attr(self_val.0, "environ")?;
                let start_response = _interp.get_attr(self_val.0, "start_response")?;
                let err_res = call_method(
                    _interp,
                    self_val.0,
                    "error_output",
                    &[environ, start_response],
                )?;
                _interp.assign_attr(self_val.0.clone(), "result", err_res)?;
                call_method(_interp, self_val.0, "finish_response", &[])?;
            }
            Ok(Value::none())
        }

        fn error_output(
            #[positional_only] self_val: PyValueRef<'_>,
            _environ: PyValue,
            start_response: PyValue,
        ) -> Result<Value> {
            let status = _interp.get_attr(self_val.0, "error_status")?;
            let headers = _interp.get_attr(self_val.0, "error_headers")?;
            // Copy headers list so mutations don't affect the class attribute.
            let hdr_copy = Value::list(_interp.collect_iterable(&headers)?);
            let ei = _interp
                .get_attr(self_val.0, "_last_exc_info")
                .unwrap_or_else(|_| Value::none());
            call_pos(_interp, start_response.0, &[status, hdr_copy, ei])?;
            let body = _interp.get_attr(self_val.0, "error_body")?;
            Ok(Value::list(vec![body]))
        }

        fn _write(#[positional_only] _self: PyValueRef<'_>, _data: PyValue) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "_write"))
        }

        fn _flush(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "_flush"))
        }

        fn get_stdin(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "get_stdin"))
        }

        fn get_stderr(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "get_stderr"))
        }

        fn add_cgi_vars(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "add_cgi_vars"))
        }
    }

    class SimpleHandler("BaseHandler") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            stdin: PyValue,
            stdout: PyValue,
            stderr: PyValue,
            environ: PyValue,
            #[default(true.into())] multithread: PyBool,
            #[default(false.into())] multiprocess: PyBool,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "stdin", stdin.0)?;
            _interp.assign_attr(self_val.0.clone(), "stdout", stdout.0)?;
            _interp.assign_attr(self_val.0.clone(), "stderr", stderr.0)?;
            _interp.assign_attr(self_val.0.clone(), "base_env", environ.0)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "wsgi_multithread",
                Value::bool_(multithread.0),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "wsgi_multiprocess",
                Value::bool_(multiprocess.0),
            )?;
            init_base_handler_attrs(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn get_stdin(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "stdin")
        }

        fn get_stderr(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "stderr")
        }

        fn add_cgi_vars(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let env = _interp.get_attr(self_val.0, "environ")?;
            let base = _interp.get_attr(self_val.0, "base_env")?;
            call_method(_interp, &env, "update", &[base])?;
            Ok(Value::none())
        }

        fn _write(#[positional_only] self_val: PyValueRef<'_>, data: PyValue) -> Result<Value> {
            let stdout = _interp.get_attr(self_val.0, "stdout")?;
            let mut remaining = match data.0.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => {
                    return Err(PyError::named("TypeError", "_write expected bytes"));
                }
            };
            while !remaining.is_empty() {
                let res = call_method(
                    _interp,
                    &stdout,
                    "write",
                    &[Value::bytes(remaining.clone())],
                )?;
                let written = res.as_int().unwrap_or(remaining.len() as i64) as usize;
                if written == 0 || written >= remaining.len() {
                    break;
                }
                remaining.drain(..written);
            }
            Ok(Value::none())
        }

        fn _flush(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let stdout = _interp.get_attr(self_val.0, "stdout")?;
            let _ = call_method(_interp, &stdout, "flush", &[])?;
            Ok(Value::none())
        }
    }

    class BaseCGIHandler("SimpleHandler") {
        constants {
            "origin_server" => Value::bool_(false),
        }
    }

    class CGIHandler("BaseCGIHandler") {
        constants {
            "wsgi_run_once" => Value::bool_(true),
            "os_environ" => Value::dict(PyDict::default()),
        }

        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let sys_mod = _interp.load_module("sys")?;
            let stdin = _interp.get_attr(&sys_mod, "stdin")?;
            let stdin_buf = _interp.get_attr(&stdin, "buffer").unwrap_or(stdin);
            let stdout = _interp.get_attr(&sys_mod, "stdout")?;
            let stdout_buf = _interp.get_attr(&stdout, "buffer").unwrap_or(stdout);
            let stderr = _interp.get_attr(&sys_mod, "stderr")?;
            let simple_cls = handlers_attr(_interp, "SimpleHandler")?;
            let init_fn = _interp.get_attr(&simple_cls, "__init__")?;
            call_pos(
                _interp,
                init_fn,
                &[
                    self_val.0.clone(),
                    stdin_buf,
                    stdout_buf,
                    stderr,
                    read_environ_dict(),
                    Value::bool_(false),
                    Value::bool_(true),
                ],
            )?;
            Ok(Value::none())
        }
    }

    class IISCGIHandler("BaseCGIHandler") {
        constants {
            "wsgi_run_once" => Value::bool_(true),
            "os_environ" => Value::dict(PyDict::default()),
        }

        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let env = read_environ_dict();
            if let Some(d) = env.as_dict() {
                let path = d
                    .get(&PyKey::str_from("PATH_INFO"))
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let script = d
                    .get(&PyKey::str_from("SCRIPT_NAME"))
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                if !script.is_empty() && path.starts_with(&script) {
                    let trimmed = &path[script.len()..];
                    env.dict_insert(PyKey::str_from("PATH_INFO"), Value::string(trimmed))?;
                }
            }
            let sys_mod = _interp.load_module("sys")?;
            let stdin = _interp.get_attr(&sys_mod, "stdin")?;
            let stdin_buf = _interp.get_attr(&stdin, "buffer").unwrap_or(stdin);
            let stdout = _interp.get_attr(&sys_mod, "stdout")?;
            let stdout_buf = _interp.get_attr(&stdout, "buffer").unwrap_or(stdout);
            let stderr = _interp.get_attr(&sys_mod, "stderr")?;
            let simple_cls = handlers_attr(_interp, "SimpleHandler")?;
            let init_fn = _interp.get_attr(&simple_cls, "__init__")?;
            call_pos(
                _interp,
                init_fn,
                &[
                    self_val.0.clone(),
                    stdin_buf,
                    stdout_buf,
                    stderr,
                    env,
                    Value::bool_(false),
                    Value::bool_(true),
                ],
            )?;
            Ok(Value::none())
        }
    }
}
