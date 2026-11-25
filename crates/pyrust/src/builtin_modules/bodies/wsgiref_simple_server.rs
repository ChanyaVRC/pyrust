// `wsgiref.simple_server` module — 100% native-Rust Python 3.12 implementation
// using PyRust's expanded macro toolkit (`pyrust_module!`, `class`,
// `link_class_bases` in `post_load_init`).
//
// Provides `ServerHandler`, `WSGIServer`, `WSGIRequestHandler`, `demo_app`,
// `make_server`, `software_version`, `server_version`, and `sys_version`.
//
// Reference: <https://docs.python.org/3/library/wsgiref.html#module-wsgiref.simple_server>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::Result;
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyInt, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

const WSGI_SIMPLE_VERSION: &str = "0.2";
const SERVER_VERSION: &str = "WSGIServer/0.2";
const SYS_VERSION: &str = "Python/3.12.3";
const SOFTWARE_VERSION: &str = "WSGIServer/0.2 Python/3.12.3";

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

fn unquote_to_latin1(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2]))
        {
            out.push(((hi << 4) | lo) as char);
            i += 3;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Wire cross-module base classes (`ServerHandler -> SimpleHandler`,
/// `WSGIServer -> http.server.HTTPServer`,
/// `WSGIRequestHandler -> http.server.BaseHTTPRequestHandler`) and set
/// `sys_version`/`software_version` from live `sys.version`.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let sys_mod = interp.load_module("sys")?;
    if let Ok(v) = interp.get_attr(&sys_mod, "version")
        && let Some(vs) = v.as_str()
    {
        let first = vs.split_whitespace().next().unwrap_or("3.12.3");
        let sv = format!("Python/{first}");
        let sw = format!("{SERVER_VERSION} {sv}");
        let mut m = module.borrow_mut();
        m.attrs
            .insert("sys_version".to_string(), Value::string(&sv));
        m.attrs
            .insert("software_version".to_string(), Value::string(&sw));
        if let Some(sh) = m.attrs.get("ServerHandler").cloned()
            && let ValueKind::PyClass(cls) = sh.kind()
        {
            cls.borrow_mut()
                .attrs
                .insert("server_software".to_string(), Value::string(&sw));
        }
    }

    let handlers_mod = interp.load_module("wsgiref.handlers")?;
    if let Ok(simple_h) = interp.get_attr(&handlers_mod, "SimpleHandler")
        && let ValueKind::PyClass(base_rc) = simple_h.kind()
        && let Some(sh) = module.borrow().attrs.get("ServerHandler").cloned()
        && let ValueKind::PyClass(sh_rc) = sh.kind()
    {
        link_class_bases(sh_rc, vec![Rc::clone(base_rc)]);
    }

    let http_srv_mod = interp.load_module("http.server")?;
    if let Ok(http_srv) = interp.get_attr(&http_srv_mod, "HTTPServer")
        && let ValueKind::PyClass(base_rc) = http_srv.kind()
        && let Some(ws) = module.borrow().attrs.get("WSGIServer").cloned()
        && let ValueKind::PyClass(ws_rc) = ws.kind()
    {
        link_class_bases(ws_rc, vec![Rc::clone(base_rc)]);
    }
    if let Ok(base_req_h) = interp.get_attr(&http_srv_mod, "BaseHTTPRequestHandler")
        && let ValueKind::PyClass(base_rc) = base_req_h.kind()
        && let Some(wrh) = module.borrow().attrs.get("WSGIRequestHandler").cloned()
        && let ValueKind::PyClass(wrh_rc) = wrh.kind()
    {
        link_class_bases(wrh_rc, vec![Rc::clone(base_rc)]);
    }

    Ok(())
}

pyrust_module! {
    constants {
        "__version__" => Value::string(WSGI_SIMPLE_VERSION),
        "__all__" => Value::list(vec![
            Value::string("WSGIServer"),
            Value::string("WSGIRequestHandler"),
            Value::string("demo_app"),
            Value::string("make_server"),
        ]),
        "server_version" => Value::string(SERVER_VERSION),
        "sys_version" => Value::string(SYS_VERSION),
        "software_version" => Value::string(SOFTWARE_VERSION),
    }

    class _StderrWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            stream: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_stream", stream.0)?;
            Ok(Value::none())
        }

        fn write(#[positional_only] self_val: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            let stream = _interp.get_attr(self_val.0, "_stream")?;
            call_method(_interp, &stream, "write", &[s.0])
        }

        fn flush(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let stream = _interp.get_attr(self_val.0, "_stream")?;
            call_method(_interp, &stream, "flush", &[])
        }

        fn writelines(#[positional_only] self_val: PyValueRef<'_>, seq: PyValue) -> Result<Value> {
            let stream = _interp.get_attr(self_val.0, "_stream")?;
            for line in _interp.collect_iterable(&seq.0)? {
                call_method(_interp, &stream, "write", &[line])?;
            }
            Ok(Value::none())
        }
    }

    class ServerHandler {
        constants {
            "server_software" => Value::string(SOFTWARE_VERSION),
            "os_environ" => Value::dict(PyDict::default()),
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let log_res: Result<()> = (|| {
                if let Ok(req_h) = _interp.get_attr(self_val.0, "request_handler")
                    && !req_h.is_none()
                {
                    let status = _interp
                        .get_attr(self_val.0, "status")?
                        .as_str()
                        .unwrap_or("500")
                        .split(' ')
                        .next()
                        .unwrap_or("500")
                        .to_string();
                    let bytes_sent = _interp
                        .get_attr(self_val.0, "bytes_sent")?
                        .as_int()
                        .unwrap_or(0);
                    let _ = call_method(
                        _interp,
                        &req_h,
                        "log_request",
                        &[Value::string(status), Value::int(bytes_sent)],
                    );
                }
                Ok(())
            })();
            let handlers_mod = _interp.load_module("wsgiref.handlers")?;
            let sh = _interp.get_attr(&handlers_mod, "SimpleHandler")?;
            let base_close = _interp.get_attr(&sh, "close")?;
            call_pos(_interp, base_close, std::slice::from_ref(self_val.0))?;
            log_res?;
            Ok(Value::none())
        }
    }

    class WSGIServer {
        constants {
            "application" => Value::none(),
        }

        fn server_bind(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let http_srv_mod = _interp.load_module("http.server")?;
            let http_srv = _interp.get_attr(&http_srv_mod, "HTTPServer")?;
            let bind_fn = _interp.get_attr(&http_srv, "server_bind")?;
            call_pos(_interp, bind_fn, std::slice::from_ref(self_val.0))?;
            call_method(_interp, self_val.0, "setup_environ", &[])?;
            Ok(Value::none())
        }

        fn setup_environ(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut env = PyDict::default();
            let server_name = _interp.get_attr(self_val.0, "server_name")?;
            let server_port = _interp.get_attr(self_val.0, "server_port")?;
            let port_str = call_pos(_interp, Value::builtin_function("str"), &[server_port])?;
            env.insert(PyKey::str_from("SERVER_NAME"), server_name);
            env.insert(PyKey::str_from("GATEWAY_INTERFACE"), Value::string("CGI/1.1"));
            env.insert(PyKey::str_from("SERVER_PORT"), port_str);
            env.insert(PyKey::str_from("REMOTE_HOST"), Value::string(""));
            env.insert(PyKey::str_from("CONTENT_LENGTH"), Value::string(""));
            env.insert(PyKey::str_from("SCRIPT_NAME"), Value::string(""));
            _interp.assign_attr(self_val.0.clone(), "base_environ", Value::dict(env))?;
            Ok(Value::none())
        }

        fn get_app(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "application")
        }

        fn set_app(
            #[positional_only] self_val: PyValueRef<'_>,
            application: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "application", application.0)?;
            Ok(Value::none())
        }
    }

    class WSGIRequestHandler {
        constants {
            "server_version" => Value::string(SERVER_VERSION),
        }

        fn get_environ(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let server = _interp.get_attr(self_val.0, "server")?;
            let base_env = _interp.get_attr(&server, "base_environ")?;
            let mut env = PyDict::default();
            if let Some(d) = base_env.as_dict() {
                for (k, v) in d.iter() {
                    env.insert(k.clone(), v.clone());
                }
            }
            let req_ver = _interp.get_attr(self_val.0, "request_version")?;
            let command = _interp.get_attr(self_val.0, "command")?;
            let raw_path = _interp
                .get_attr(self_val.0, "path")?
                .as_str()
                .unwrap_or("/")
                .to_string();
            let (path, query) = match raw_path.split_once('?') {
                Some((p, q)) => (p.to_string(), q.to_string()),
                None => (raw_path, String::new()),
            };
            env.insert(PyKey::str_from("SERVER_PROTOCOL"), req_ver);
            let http_ver = _interp
                .get_attr(self_val.0, "protocol_version")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "HTTP/1.0".to_string());
            env.insert(PyKey::str_from("SERVER_SOFTWARE"), Value::string(SOFTWARE_VERSION));
            let _ = http_ver;
            env.insert(PyKey::str_from("REQUEST_METHOD"), command);
            env.insert(
                PyKey::str_from("PATH_INFO"),
                Value::string(unquote_to_latin1(&path)),
            );
            env.insert(PyKey::str_from("QUERY_STRING"), Value::string(query));

            let client_addr = _interp.get_attr(self_val.0, "client_address")?;
            let addr_parts = _interp.collect_iterable(&client_addr)?;
            let remote_addr = addr_parts
                .first()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            env.insert(PyKey::str_from("REMOTE_ADDR"), Value::string(&remote_addr));
            env.insert(PyKey::str_from("REMOTE_HOST"), Value::string(&remote_addr));

            let headers = _interp.get_attr(self_val.0, "headers")?;
            let ct = call_method(_interp, &headers, "get", &[Value::string("content-type")])?;
            if let Some(ct_s) = ct.as_str() {
                env.insert(PyKey::str_from("CONTENT_TYPE"), Value::string(ct_s));
            } else {
                env.insert(PyKey::str_from("CONTENT_TYPE"), Value::string("text/plain"));
            }
            let cl = call_method(_interp, &headers, "get", &[Value::string("content-length")])?;
            if let Some(cl_s) = cl.as_str() {
                env.insert(PyKey::str_from("CONTENT_LENGTH"), Value::string(cl_s));
            }

            let items = call_method(_interp, &headers, "items", &[])?;
            for item in _interp.collect_iterable(&items)? {
                let pair = _interp.collect_iterable(&item)?;
                if pair.len() != 2 {
                    continue;
                }
                let Some(k) = pair[0].as_str() else {
                    continue;
                };
                let Some(v) = pair[1].as_str() else {
                    continue;
                };
                let key = format!("HTTP_{}", k.replace('-', "_").to_ascii_uppercase());
                let val = v.trim();
                if key == "HTTP_CONTENT_TYPE" || key == "HTTP_CONTENT_LENGTH" {
                    continue;
                }
                let pk = PyKey::str_from(&key);
                if let Some(existing) = env.get(&pk).and_then(|e| e.as_str().map(str::to_string)) {
                    env.insert(pk, Value::string(format!("{existing},{val}")));
                } else {
                    env.insert(pk, Value::string(val));
                }
            }
            Ok(Value::dict(env))
        }

        fn get_stderr(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let sys_mod = _interp.load_module("sys")?;
            let stderr = _interp.get_attr(&sys_mod, "stderr")?;
            if _interp.get_attr(&stderr, "writelines").is_ok() {
                return Ok(stderr);
            }
            let ss_mod = _interp.load_module("wsgiref.simple_server")?;
            let wrap_cls = _interp.get_attr(&ss_mod, "_StderrWrapper")?;
            call_pos(_interp, wrap_cls, &[stderr])
        }

        fn handle(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let rfile = _interp.get_attr(self_val.0, "rfile")?;
            let raw_line = call_method(_interp, &rfile, "readline", &[Value::int(65537)])?;
            _interp.assign_attr(self_val.0.clone(), "raw_requestline", raw_line.clone())?;
            let raw_len = match raw_line.kind() {
                ValueKind::Bytes(b) => b.len(),
                _ => 0,
            };
            if raw_len > 65536 {
                _interp.assign_attr(self_val.0.clone(), "requestline", Value::string(""))?;
                _interp.assign_attr(self_val.0.clone(), "request_version", Value::string(""))?;
                _interp.assign_attr(self_val.0.clone(), "command", Value::string(""))?;
                call_method(_interp, self_val.0, "send_error", &[Value::int(414)])?;
                return Ok(Value::none());
            }
            let parsed = call_method(_interp, self_val.0, "parse_request", &[])?;
            if !_interp.truthy_value(&parsed)? {
                return Ok(Value::none());
            }
            let ss_mod = _interp.load_module("wsgiref.simple_server")?;
            let sh_cls = _interp.get_attr(&ss_mod, "ServerHandler")?;
            let wfile = _interp.get_attr(self_val.0, "wfile")?;
            let stderr = call_method(_interp, self_val.0, "get_stderr", &[])?;
            let env = call_method(_interp, self_val.0, "get_environ", &[])?;
            let handler = call_pos(_interp, sh_cls, &[rfile, wfile, stderr, env])?;
            _interp.assign_attr(handler.clone(), "request_handler", self_val.0.clone())?;
            let server = _interp.get_attr(self_val.0, "server")?;
            let app = call_method(_interp, &server, "get_app", &[])?;
            call_method(_interp, &handler, "run", &[app])?;
            Ok(Value::none())
        }
    }

    fn demo_app(environ: PyValue, start_response: PyValue) -> Result<Value> {
        let mut out = String::from("Hello world!\n\n");
        let mut items: Vec<(String, Value)> = Vec::new();
        if let Some(d) = environ.0.as_dict() {
            for (k, v) in d.iter() {
                if let PyKey::Str(sv) = k
                    && let Some(ks) = sv.as_str()
                {
                    items.push((ks.to_string(), v.clone()));
                }
            }
        } else {
            let it = call_method(_interp, &environ.0, "items", &[])?;
            for entry in _interp.collect_iterable(&it)? {
                let pair = _interp.collect_iterable(&entry)?;
                if pair.len() == 2
                    && let Some(ks) = pair[0].as_str()
                {
                    items.push((ks.to_string(), pair[1].clone()));
                }
            }
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in items {
            let r = render_value_repr(_interp, &v)?;
            out.push_str(&format!("{k} = {r}\n"));
        }
        let hdrs = Value::list(vec![Value::tuple(vec![
            Value::string("Content-Type"),
            Value::string("text/plain; charset=utf-8"),
        ])]);
        call_pos(_interp, start_response.0, &[Value::string("200 OK"), hdrs])?;
        Ok(Value::list(vec![Value::bytes(out.into_bytes())]))
    }

    fn make_server(
        host: PyStr<'_>,
        port: PyInt,
        app: PyValue,
        #[default(OptionalArg::Omitted)] server_class: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] handler_class: OptValueRef<'_>,
    ) -> Result<Value> {
        let port_i = port.expect_i64(FN_NAME, "port")?;
        let ss_mod = _interp.load_module("wsgiref.simple_server")?;
        let srv_cls = match server_class.as_value() {
            Some(v) if !v.is_none() => v.clone(),
            _ => _interp.get_attr(&ss_mod, "WSGIServer")?,
        };
        let hnd_cls = match handler_class.as_value() {
            Some(v) if !v.is_none() => v.clone(),
            _ => _interp.get_attr(&ss_mod, "WSGIRequestHandler")?,
        };
        let addr = Value::tuple(vec![Value::string(host.0.as_ref()), Value::int(port_i)]);
        let server = call_pos(_interp, srv_cls, &[addr, hnd_cls])?;
        call_method(_interp, &server, "set_app", &[app.0])?;
        Ok(server)
    }
}
