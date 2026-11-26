// `poplib` module — 100% native-Rust Python 3.12 POP3 and POP3-over-SSL client
// implementation built over `socket` and `ssl` using PyRust's macro toolkit
// (issue #3258).
//
// Reference: <https://docs.python.org/3.12/library/poplib.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBytesLike, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

const POP3_PORT_VAL: i64 = 110;
const POP3_SSL_PORT_VAL: i64 = 995;
const CR_BYTES: &[u8] = b"\r";
const LF_BYTES: &[u8] = b"\n";
const CRLF_BYTES: &[u8] = b"\r\n";
const MAXLINE_VAL: i64 = 2048;

// ── Shared helpers ───────────────────────────────────────────────────────────

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded = Vec::with_capacity(pos.len() + kw.len());
    for v in pos {
        expanded.push(ExpandedCallArg {
            name: None,
            value: v.clone(),
        });
    }
    for &(k, ref v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some(k.to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_pos(interp, m, args)
}

fn py_getitem(interp: &mut Interpreter, obj: &Value, key: Value) -> Result<Value> {
    call_method(interp, obj, "__getitem__", &[key])
}

fn call_method_kw(
    interp: &mut Interpreter,
    obj: &Value,
    name: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_kw(interp, m, pos, kw)
}

fn raise_error_proto_str(interp: &mut Interpreter, msg: &str) -> PyError {
    if let Ok(m) = interp.load_module("poplib")
        && let Ok(cls) = interp.get_attr(&m, "error_proto")
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(msg)])
    {
        return PyError::raised(inst);
    }
    PyError::named("Exception", msg)
}

fn raise_error_proto_val(interp: &mut Interpreter, val: Value) -> PyError {
    if let Ok(m) = interp.load_module("poplib")
        && let Ok(cls) = interp.get_attr(&m, "error_proto")
        && let Ok(inst) = call_pos(interp, cls, std::slice::from_ref(&val))
    {
        return PyError::raised(inst);
    }
    PyError::named("Exception", format!("{val:?}"))
}

fn global_default_timeout(interp: &mut Interpreter) -> Result<Value> {
    let sock_mod = interp.load_module("socket")?;
    interp.get_attr(&sock_mod, "_GLOBAL_DEFAULT_TIMEOUT")
}

fn value_to_str(interp: &mut Interpreter, val: &Value) -> Result<String> {
    if let Some(s) = val.as_str() {
        return Ok(s.to_string());
    }
    let sv = call_pos(
        interp,
        Value::builtin_function("str"),
        std::slice::from_ref(val),
    )?;
    Ok(sv.as_str().unwrap_or_default().to_string())
}

fn get_int_attr(interp: &mut Interpreter, obj: &Value, name: &str, default: i64) -> Result<i64> {
    let v = interp.get_attr(obj, name)?;
    Ok(v.as_int().unwrap_or(default))
}

fn get_str_attr(
    interp: &mut Interpreter,
    obj: &Value,
    name: &str,
    default: &str,
) -> Result<String> {
    let v = interp.get_attr(obj, name)?;
    Ok(v.as_str().unwrap_or(default).to_string())
}

fn print_debug(interp: &mut Interpreter, tag: &str, val: &Value) -> Result<()> {
    let r = render_value_repr(interp, val)?;
    call_pos(
        interp,
        Value::builtin_function("print"),
        &[Value::string(tag), Value::string(r)],
    )?;
    Ok(())
}

fn pop3_init_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    host: &str,
    port: i64,
    timeout: Value,
) -> Result<()> {
    interp.assign_attr(self_val.clone(), "host", Value::string(host))?;
    interp.assign_attr(self_val.clone(), "port", Value::int(port))?;
    interp.assign_attr(self_val.clone(), "_tls_established", Value::bool_(false))?;
    let sock = call_method(interp, self_val, "_create_socket", &[timeout])?;
    let file = call_method(interp, &sock, "makefile", &[Value::string("rb")])?;
    interp.assign_attr(self_val.clone(), "sock", sock)?;
    interp.assign_attr(self_val.clone(), "file", file)?;
    interp.assign_attr(self_val.clone(), "_debugging", Value::int(0))?;
    let welcome = call_method(interp, self_val, "_getresp", &[])?;
    interp.assign_attr(self_val.clone(), "welcome", welcome)?;
    Ok(())
}

fn pop3_create_socket_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    timeout: Value,
) -> Result<Value> {
    if !timeout.is_none() && !interp.truthy_value(&timeout)? {
        return Err(PyError::named(
            "ValueError",
            "Non-blocking socket (timeout=0) is not supported",
        ));
    }
    let host = interp.get_attr(self_val, "host")?;
    let port = interp.get_attr(self_val, "port")?;
    let sock_mod = interp.load_module("socket")?;
    let create_conn = interp.get_attr(&sock_mod, "create_connection")?;
    call_pos(
        interp,
        create_conn,
        &[Value::tuple(vec![host, port]), timeout],
    )
}

/// Extract the `<...>` timestamp from a POP3 `+OK` welcome banner (matching
/// CPython's `re.compile(br'\+OK.[^<]*(<.*>)')`).
fn extract_apop_timestamp(welcome: &[u8]) -> Option<&[u8]> {
    if welcome.len() < 4 || &welcome[..3] != b"+OK" {
        return None;
    }
    // The 4th byte must not be `\n` (since `.` in regex does not match `\n`).
    if welcome[3] == b'\n' {
        return None;
    }
    let rest = &welcome[4..];
    let lt = rest.iter().position(|&b| b == b'<')?;
    // Find last `>` on the same line (greedy `.*>`).
    let after_lt = &rest[lt..];
    let line_end = after_lt
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(after_lt.len());
    let same_line = &after_lt[..line_end];
    let gt = same_line.iter().rposition(|&b| b == b'>')?;
    Some(&same_line[..=gt])
}

// ── Post-load initialization ─────────────────────────────────────────────────

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // Attach compiled `POP3.timestamp` regex attribute matching CPython 3.12.
    let re_mod = interp.load_module("re")?;
    let compile_fn = interp.get_attr(&re_mod, "compile")?;
    let pat = call_pos(
        interp,
        compile_fn,
        &[Value::bytes(br"\+OK.[^<]*(<.*>)".to_vec())],
    )?;
    if let Some(pop3_val) = module.borrow().attrs.get("POP3").cloned()
        && let ValueKind::PyClass(pop3_rc) = pop3_val.kind()
    {
        pop3_rc
            .borrow_mut()
            .attrs
            .insert("timestamp".to_string(), pat);
    }
    Ok(())
}

// ── Native module definition ─────────────────────────────────────────────────

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("POP3"),
            Value::string("error_proto"),
            Value::string("POP3_SSL"),
        ]),
        "POP3_PORT" => Value::int(POP3_PORT_VAL),
        "POP3_SSL_PORT" => Value::int(POP3_SSL_PORT_VAL),
        "CR" => Value::bytes(CR_BYTES.to_vec()),
        "LF" => Value::bytes(LF_BYTES.to_vec()),
        "CRLF" => Value::bytes(CRLF_BYTES.to_vec()),
        "_MAXLINE" => Value::int(MAXLINE_VAL),
        "HAVE_SSL" => Value::bool_(true),
    }

    /// Exception raised when an error or invalid response is received.
    exception error_proto;

    // ── POP3 ─────────────────────────────────────────────────────────────────

    class POP3 {
        constants {
            "encoding" => Value::string("UTF-8"),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyStr<'_>,
            #[default(POP3_PORT_VAL.into())] port: PyIndex,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let timeout_val = match timeout.as_value() {
                Some(v) => v.clone(),
                None => global_default_timeout(_interp)?,
            };
            pop3_init_impl(_interp, self_val.0, &host.0, port_i64, timeout_val)?;
            Ok(Value::none())
        }

        fn _create_socket(
            #[positional_only] self_val: PyValueRef<'_>,
            timeout: PyValue,
        ) -> Result<Value> {
            pop3_create_socket_impl(_interp, self_val.0, timeout.0)
        }

        fn _putline(
            #[positional_only] self_val: PyValueRef<'_>,
            line: PyBytesLike<'_>,
        ) -> Result<Value> {
            let dbg = get_int_attr(_interp, self_val.0, "_debugging", 0)?;
            let raw = line.as_bytes();
            if dbg > 1 {
                print_debug(_interp, "*put*", &Value::bytes(raw.to_vec()))?;
            }
            let mut full = Vec::with_capacity(raw.len() + 2);
            full.extend_from_slice(raw);
            full.extend_from_slice(CRLF_BYTES);
            let sock = _interp.get_attr(self_val.0, "sock")?;
            call_method(_interp, &sock, "sendall", &[Value::bytes(full)])?;
            Ok(Value::none())
        }

        fn _putcmd(
            #[positional_only] self_val: PyValueRef<'_>,
            line: PyStr<'_>,
        ) -> Result<Value> {
            let dbg = get_int_attr(_interp, self_val.0, "_debugging", 0)?;
            if dbg != 0 {
                print_debug(_interp, "*cmd*", &Value::string(&*line.0))?;
            }
            let encoding = get_str_attr(_interp, self_val.0, "encoding", "UTF-8")?;
            let encoded = call_method(
                _interp,
                &Value::string(&*line.0),
                "encode",
                &[Value::string(encoding)],
            )?;
            call_method(_interp, self_val.0, "_putline", &[encoded])?;
            Ok(Value::none())
        }

        fn _getline(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let file = _interp.get_attr(self_val.0, "file")?;
            let line_val = call_method(_interp, &file, "readline", &[Value::int(MAXLINE_VAL + 1)])?;
            let line = match line_val.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => Vec::new(),
            };
            if (line.len() as i64) > MAXLINE_VAL {
                return Err(raise_error_proto_str(_interp, "line too long"));
            }
            let dbg = get_int_attr(_interp, self_val.0, "_debugging", 0)?;
            if dbg > 1 {
                print_debug(_interp, "*get*", &Value::bytes(line.clone()))?;
            }
            if line.is_empty() {
                return Err(raise_error_proto_str(_interp, "-ERR EOF"));
            }
            let octets = line.len() as i64;
            let stripped = if line.ends_with(CRLF_BYTES) {
                line[..line.len() - 2].to_vec()
            } else if line.first() == Some(&b'\r') {
                line[1..line.len() - 1].to_vec()
            } else {
                line[..line.len() - 1].to_vec()
            };
            Ok(Value::tuple(vec![Value::bytes(stripped), Value::int(octets)]))
        }

        fn _getresp(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pair = call_method(_interp, self_val.0, "_getline", &[])?;
            let resp = py_getitem(_interp, &pair, Value::int(0))?;
            let dbg = get_int_attr(_interp, self_val.0, "_debugging", 0)?;
            if dbg > 1 {
                print_debug(_interp, "*resp*", &resp)?;
            }
            let ok = match resp.kind() {
                ValueKind::Bytes(b) => b.starts_with(b"+"),
                _ => false,
            };
            if !ok {
                return Err(raise_error_proto_val(_interp, resp));
            }
            Ok(resp)
        }

        fn _getlongresp(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let resp = call_method(_interp, self_val.0, "_getresp", &[])?;
            let mut lines = Vec::new();
            let mut octets: i64 = 0;
            loop {
                let pair = call_method(_interp, self_val.0, "_getline", &[])?;
                let line_val = py_getitem(_interp, &pair, Value::int(0))?;
                let mut o = py_getitem(_interp, &pair, Value::int(1))?
                    .as_int()
                    .unwrap_or(0);
                let raw = match line_val.kind() {
                    ValueKind::Bytes(b) => b.as_ref().clone(),
                    _ => Vec::new(),
                };
                if raw == b"." {
                    break;
                }
                let final_line = if raw.starts_with(b"..") {
                    o -= 1;
                    raw[1..].to_vec()
                } else {
                    raw
                };
                octets += o;
                lines.push(Value::bytes(final_line));
            }
            Ok(Value::tuple(vec![
                resp,
                Value::list(lines),
                Value::int(octets),
            ]))
        }

        fn _shortcmd(
            #[positional_only] self_val: PyValueRef<'_>,
            line: PyStr<'_>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "_putcmd", &[Value::string(&*line.0)])?;
            call_method(_interp, self_val.0, "_getresp", &[])
        }

        fn _longcmd(
            #[positional_only] self_val: PyValueRef<'_>,
            line: PyStr<'_>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "_putcmd", &[Value::string(&*line.0)])?;
            call_method(_interp, self_val.0, "_getlongresp", &[])
        }

        fn getwelcome(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "welcome")
        }

        fn set_debuglevel(
            #[positional_only] self_val: PyValueRef<'_>,
            level: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_debugging", level.0)?;
            Ok(Value::none())
        }

        fn user(#[positional_only] self_val: PyValueRef<'_>, user: PyValue) -> Result<Value> {
            let u = value_to_str(_interp, &user.0)?;
            call_method(
                _interp,
                self_val.0,
                "_shortcmd",
                &[Value::string(format!("USER {u}"))],
            )
        }

        fn pass_(#[positional_only] self_val: PyValueRef<'_>, pswd: PyValue) -> Result<Value> {
            let p = value_to_str(_interp, &pswd.0)?;
            call_method(
                _interp,
                self_val.0,
                "_shortcmd",
                &[Value::string(format!("PASS {p}"))],
            )
        }

        fn stat(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let retval = call_method(_interp, self_val.0, "_shortcmd", &[Value::string("STAT")])?;
            let rets = call_method(_interp, &retval, "split", &[])?;
            let dbg = get_int_attr(_interp, self_val.0, "_debugging", 0)?;
            if dbg != 0 {
                print_debug(_interp, "*stat*", &rets)?;
            }
            let num_val = py_getitem(_interp, &rets, Value::int(1))?;
            let size_val = py_getitem(_interp, &rets, Value::int(2))?;
            let num = call_pos(_interp, Value::builtin_function("int"), &[num_val])?;
            let size = call_pos(_interp, Value::builtin_function("int"), &[size_val])?;
            Ok(Value::tuple(vec![num, size]))
        }

        fn list(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] which: OptValueRef<'_>,
        ) -> Result<Value> {
            if let Some(w) = which.as_value()
                && !w.is_none()
            {
                let ws = value_to_str(_interp, w)?;
                return call_method(
                    _interp,
                    self_val.0,
                    "_shortcmd",
                    &[Value::string(format!("LIST {ws}"))],
                );
            }
            call_method(_interp, self_val.0, "_longcmd", &[Value::string("LIST")])
        }

        fn retr(#[positional_only] self_val: PyValueRef<'_>, which: PyValue) -> Result<Value> {
            let ws = value_to_str(_interp, &which.0)?;
            call_method(
                _interp,
                self_val.0,
                "_longcmd",
                &[Value::string(format!("RETR {ws}"))],
            )
        }

        fn dele(#[positional_only] self_val: PyValueRef<'_>, which: PyValue) -> Result<Value> {
            let ws = value_to_str(_interp, &which.0)?;
            call_method(
                _interp,
                self_val.0,
                "_shortcmd",
                &[Value::string(format!("DELE {ws}"))],
            )
        }

        fn noop(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "_shortcmd", &[Value::string("NOOP")])
        }

        fn rset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "_shortcmd", &[Value::string("RSET")])
        }

        fn quit(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let resp = call_method(_interp, self_val.0, "_shortcmd", &[Value::string("QUIT")])?;
            call_method(_interp, self_val.0, "close", &[])?;
            Ok(resp)
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let file = _interp.get_attr(self_val.0, "file")?;
            _interp.assign_attr(self_val.0.clone(), "file", Value::none())?;
            let file_res = if !file.is_none() {
                call_method(_interp, &file, "close", &[])
            } else {
                Ok(Value::none())
            };
            let sock = _interp.get_attr(self_val.0, "sock")?;
            _interp.assign_attr(self_val.0.clone(), "sock", Value::none())?;
            let sock_res = if !sock.is_none() {
                let sock_mod = _interp.load_module("socket")?;
                let shut_rdwr = _interp.get_attr(&sock_mod, "SHUT_RDWR")?;
                let _ = call_method(_interp, &sock, "shutdown", &[shut_rdwr]);
                call_method(_interp, &sock, "close", &[])
            } else {
                Ok(Value::none())
            };
            file_res?;
            sock_res?;
            Ok(Value::none())
        }

        fn rpop(#[positional_only] self_val: PyValueRef<'_>, user: PyValue) -> Result<Value> {
            let u = value_to_str(_interp, &user.0)?;
            call_method(
                _interp,
                self_val.0,
                "_shortcmd",
                &[Value::string(format!("RPOP {u}"))],
            )
        }

        fn apop(
            #[positional_only] self_val: PyValueRef<'_>,
            user: PyValue,
            password: PyStr<'_>,
        ) -> Result<Value> {
            let encoding = get_str_attr(_interp, self_val.0, "encoding", "UTF-8")?;
            let secret_val = call_method(
                _interp,
                &Value::string(&*password.0),
                "encode",
                &[Value::string(encoding)],
            )?;
            let secret_bytes = match secret_val.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => Vec::new(),
            };
            let welcome = _interp.get_attr(self_val.0, "welcome")?;
            let welcome_bytes = match welcome.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => Vec::new(),
            };
            let Some(ts) = extract_apop_timestamp(&welcome_bytes) else {
                return Err(raise_error_proto_str(
                    _interp,
                    "-ERR APOP not supported by server",
                ));
            };
            let mut combined = Vec::with_capacity(ts.len() + secret_bytes.len());
            combined.extend_from_slice(ts);
            combined.extend_from_slice(&secret_bytes);

            let hashlib_mod = _interp.load_module("hashlib")?;
            let md5_fn = _interp.get_attr(&hashlib_mod, "md5")?;
            let h = call_pos(_interp, md5_fn, &[Value::bytes(combined)])?;
            let digest_val = call_method(_interp, &h, "hexdigest", &[])?;
            let digest = value_to_str(_interp, &digest_val)?;
            let u = value_to_str(_interp, &user.0)?;
            call_method(
                _interp,
                self_val.0,
                "_shortcmd",
                &[Value::string(format!("APOP {u} {digest}"))],
            )
        }

        fn top(
            #[positional_only] self_val: PyValueRef<'_>,
            which: PyValue,
            howmuch: PyValue,
        ) -> Result<Value> {
            let ws = value_to_str(_interp, &which.0)?;
            let hs = value_to_str(_interp, &howmuch.0)?;
            call_method(
                _interp,
                self_val.0,
                "_longcmd",
                &[Value::string(format!("TOP {ws} {hs}"))],
            )
        }

        fn uidl(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] which: OptValueRef<'_>,
        ) -> Result<Value> {
            if let Some(w) = which.as_value()
                && !w.is_none()
            {
                let ws = value_to_str(_interp, w)?;
                return call_method(
                    _interp,
                    self_val.0,
                    "_shortcmd",
                    &[Value::string(format!("UIDL {ws}"))],
                );
            }
            call_method(_interp, self_val.0, "_longcmd", &[Value::string("UIDL")])
        }

        fn utf8(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "_shortcmd", &[Value::string("UTF8")])
        }

        fn capa(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let resp = match call_method(_interp, self_val.0, "_longcmd", &[Value::string("CAPA")]) {
                Ok(r) => r,
                Err(_) => {
                    return Err(raise_error_proto_str(
                        _interp,
                        "-ERR CAPA not supported by server",
                    ));
                }
            };
            let rawcaps = py_getitem(_interp, &resp, Value::int(1))?;
            let cap_lines = _interp.collect_iterable(&rawcaps)?;
            let mut caps = PyDict::default();
            for line_val in cap_lines {
                let raw = match line_val.kind() {
                    ValueKind::Bytes(b) => b.as_ref().clone(),
                    _ => Vec::new(),
                };
                let text = std::str::from_utf8(&raw).map_err(|_| {
                    PyError::named(
                        "UnicodeDecodeError",
                        "ascii codec can't decode byte in CAPA response",
                    )
                })?;
                let mut parts = text.split_whitespace();
                if let Some(name) = parts.next() {
                    let args: Vec<Value> = parts.map(Value::string).collect();
                    caps.insert(PyKey::str_from(name), Value::list(args));
                }
            }
            Ok(Value::dict(caps))
        }

        fn stls(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            let tls_est = {
                let v = _interp.get_attr(self_val.0, "_tls_established")?;
                _interp.truthy_value(&v)?
            };
            if tls_est {
                return Err(raise_error_proto_str(
                    _interp,
                    "-ERR TLS session already established",
                ));
            }
            let caps = call_method(_interp, self_val.0, "capa", &[])?;
            let has_stls = caps
                .as_dict()
                .is_some_and(|d| d.contains_key(&PyKey::str_from("STLS")));
            if !has_stls {
                return Err(raise_error_proto_str(
                    _interp,
                    "-ERR STLS not supported by server",
                ));
            }
            let ctx = match context.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let ssl_mod = _interp.load_module("ssl")?;
                    let mk = _interp.get_attr(&ssl_mod, "_create_stdlib_context")?;
                    call_pos(_interp, mk, &[])?
                }
            };
            let resp = call_method(_interp, self_val.0, "_shortcmd", &[Value::string("STLS")])?;
            let sock = _interp.get_attr(self_val.0, "sock")?;
            let host = _interp.get_attr(self_val.0, "host")?;
            let wrapped = call_method_kw(
                _interp,
                &ctx,
                "wrap_socket",
                &[sock],
                &[("server_hostname", host)],
            )?;
            let file = call_method(_interp, &wrapped, "makefile", &[Value::string("rb")])?;
            _interp.assign_attr(self_val.0.clone(), "sock", wrapped)?;
            _interp.assign_attr(self_val.0.clone(), "file", file)?;
            _interp.assign_attr(self_val.0.clone(), "_tls_established", Value::bool_(true))?;
            Ok(resp)
        }
    }

    // ── POP3_SSL ─────────────────────────────────────────────────────────────

    class POP3_SSL("POP3") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyStr<'_>,
            #[default(POP3_SSL_PORT_VAL.into())] port: PyIndex,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            timeout: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            context: OptValueRef<'_>,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let ctx = match context.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let ssl_mod = _interp.load_module("ssl")?;
                    let mk = _interp.get_attr(&ssl_mod, "_create_stdlib_context")?;
                    call_pos(_interp, mk, &[])?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "context", ctx)?;
            let timeout_val = match timeout.as_value() {
                Some(v) => v.clone(),
                None => global_default_timeout(_interp)?,
            };
            pop3_init_impl(_interp, self_val.0, &host.0, port_i64, timeout_val)?;
            Ok(Value::none())
        }

        fn _create_socket(
            #[positional_only] self_val: PyValueRef<'_>,
            timeout: PyValue,
        ) -> Result<Value> {
            let sock = pop3_create_socket_impl(_interp, self_val.0, timeout.0)?;
            let ctx = _interp.get_attr(self_val.0, "context")?;
            let host = _interp.get_attr(self_val.0, "host")?;
            call_method_kw(
                _interp,
                &ctx,
                "wrap_socket",
                &[sock],
                &[("server_hostname", host)],
            )
        }

        fn stls(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _context: OptValueRef<'_>,
        ) -> Result<Value> {
            Err(raise_error_proto_str(
                _interp,
                "-ERR TLS session already established",
            ))
        }
    }
}
