// `ftplib` module — 100% native-Rust Python 3.12 FTP and FTP-over-TLS client
// implementation built over `socket` and `ssl` using PyRust's macro toolkit
// (issue #3258).
//
// Reference: <https://docs.python.org/3.12/library/ftplib.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::set_native_state;
use crate::error::{PyError, PyErrorKind, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, lookup_exc_class, render_value_repr, value_type_name_str,
};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, PyModule, Value, ValueKind};

const MSG_OOB_VAL: i64 = 0x1;
const FTP_PORT_VAL: i64 = 21;
const MAXLINE_VAL: i64 = 8192;
const CRLF_STR: &str = "\r\n";
const B_CRLF_BYTES: &[u8] = b"\r\n";

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

fn ftplib_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("ftplib")?;
    interp.get_attr(&m, name)
}

fn raise_ftplib_err(interp: &mut Interpreter, exc_name: &str, msg: &str) -> PyError {
    if let Ok(cls) = ftplib_attr(interp, exc_name)
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(msg)])
    {
        return PyError::raised(inst);
    }
    PyError::named("Exception", msg)
}

fn global_default_timeout(interp: &mut Interpreter) -> Result<Value> {
    let sock_mod = interp.load_module("socket")?;
    interp.get_attr(&sock_mod, "_GLOBAL_DEFAULT_TIMEOUT")
}

fn py_getitem(interp: &mut Interpreter, obj: &Value, key: Value) -> Result<Value> {
    call_method(interp, obj, "__getitem__", &[key])
}

fn is_same_object(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    Ok(interp
        .eval_binary(a.clone(), crate::ast::BinaryOp::Is, b.clone())?
        .as_bool())
}

fn is_instance(interp: &mut Interpreter, obj: &Value, cls: Value) -> Result<bool> {
    let res = call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), cls],
    )?;
    interp.truthy_value(&res)
}

fn is_ssl_socket(interp: &mut Interpreter, obj: &Value) -> Result<bool> {
    let ssl_mod = interp.load_module("ssl")?;
    let ssl_sock_cls = interp.get_attr(&ssl_mod, "SSLSocket")?;
    is_instance(interp, obj, ssl_sock_cls)
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

fn get_int_attr(interp: &mut Interpreter, obj: &Value, name: &str, default: i64) -> Result<i64> {
    let v = interp.get_attr(obj, name)?;
    Ok(v.as_int().unwrap_or(default))
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

fn sanitize_str(interp: &mut Interpreter, val: &Value) -> Result<String> {
    if let Some(s) = val.as_str() {
        let prefix: String = s.chars().take(5).collect();
        if prefix == "pass " || prefix == "PASS " {
            let trimmed = s.trim_end_matches(['\r', '\n']);
            let stars = "*".repeat(trimmed.len().saturating_sub(5));
            let suffix = &s[trimmed.len()..];
            let masked = Value::string(format!("{prefix}{stars}{suffix}"));
            return render_value_repr(interp, &masked);
        }
    }
    render_value_repr(interp, val)
}

fn print_debug_pair(interp: &mut Interpreter, tag: &str, val: &Value) -> Result<()> {
    let san = call_method(interp, val, "__str__", &[]);
    let _ = san;
    call_pos(
        interp,
        Value::builtin_function("print"),
        &[Value::string(tag), val.clone()],
    )?;
    Ok(())
}

fn unwrap_and_close_ssl_conn(interp: &mut Interpreter, conn: &Value) -> Result<()> {
    if is_ssl_socket(interp, conn)? {
        let unwrapped = call_method(interp, conn, "unwrap", &[])?;
        let _ = call_method(interp, &unwrapped, "close", &[]);
    }
    Ok(())
}

// ── Response Parsers ─────────────────────────────────────────────────────────

fn parse150_impl(interp: &mut Interpreter, resp: &str) -> Result<Value> {
    if !resp.starts_with("150") {
        return Err(raise_ftplib_err(interp, "error_reply", resp));
    }
    // Match `150 .* \((\d+) bytes\)` case-insensitively with ASCII digits.
    let bytes = resp.as_bytes();
    if bytes.len() < 4 || bytes[3] != b' ' {
        return Ok(Value::none());
    }
    let lower = resp.to_ascii_lowercase();
    let suffix = " bytes)";
    let mut search_start = 4;
    while let Some(rel_end) = lower[search_start..].find(suffix) {
        let end_idx = search_start + rel_end;
        // Walk backwards over ASCII digits from end_idx.
        let mut digit_start = end_idx;
        while digit_start > 4 && bytes[digit_start - 1].is_ascii_digit() {
            digit_start -= 1;
        }
        if digit_start < end_idx && digit_start > 4 && bytes[digit_start - 1] == b'(' {
            let num_str = &resp[digit_start..end_idx];
            if let Ok(n) = num_str.parse::<i64>() {
                return Ok(Value::int(n));
            }
        }
        search_start = end_idx + 1;
    }
    Ok(Value::none())
}

fn parse227_impl(interp: &mut Interpreter, resp: &str) -> Result<(String, i64)> {
    if !resp.starts_with("227") {
        return Err(raise_ftplib_err(interp, "error_reply", resp));
    }
    // Search for `(\d+),(\d+),(\d+),(\d+),(\d+),(\d+)` (ASCII digits).
    let b = resp.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        if b[i].is_ascii_digit() {
            let mut pos = i;
            let mut nums = [0i64; 6];
            let mut num_strs = [""; 6];
            let mut ok = true;
            for idx in 0..6 {
                let start = pos;
                while pos < n && b[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos == start {
                    ok = false;
                    break;
                }
                num_strs[idx] = &resp[start..pos];
                match num_strs[idx].parse::<i64>() {
                    Ok(v) => nums[idx] = v,
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
                if idx < 5 {
                    if pos < n && b[pos] == b',' {
                        pos += 1;
                    } else {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                let host = format!(
                    "{}.{}.{}.{}",
                    num_strs[0], num_strs[1], num_strs[2], num_strs[3]
                );
                let port = (nums[4] << 8) + nums[5];
                return Ok((host, port));
            }
        }
        i += 1;
    }
    Err(raise_ftplib_err(interp, "error_proto", resp))
}

fn parse229_impl(interp: &mut Interpreter, resp: &str, peer: &Value) -> Result<(Value, i64)> {
    if !resp.starts_with("229") {
        return Err(raise_ftplib_err(interp, "error_reply", resp));
    }
    let Some(left) = resp.find('(') else {
        return Err(raise_ftplib_err(interp, "error_proto", resp));
    };
    let Some(right_rel) = resp[left + 1..].find(')') else {
        return Err(raise_ftplib_err(interp, "error_proto", resp));
    };
    let right = left + 1 + right_rel;
    let inner = &resp[left + 1..right];
    let mut chars = inner.chars();
    let Some(first) = chars.next() else {
        return Err(raise_ftplib_err(interp, "error_proto", resp));
    };
    let Some(last) = inner.chars().next_back() else {
        return Err(raise_ftplib_err(interp, "error_proto", resp));
    };
    if first != last {
        return Err(raise_ftplib_err(interp, "error_proto", resp));
    }
    let parts: Vec<&str> = inner.split(first).collect();
    if parts.len() != 5 {
        return Err(raise_ftplib_err(interp, "error_proto", resp));
    }
    let port: i64 = parts[3].parse().map_err(|_| {
        PyError::named(
            "ValueError",
            format!("invalid literal for int(): {:?}", parts[3]),
        )
    })?;
    let host = py_getitem(interp, peer, Value::int(0))?;
    Ok((host, port))
}

fn parse257_impl(interp: &mut Interpreter, resp: &str) -> Result<String> {
    if !resp.starts_with("257") {
        return Err(raise_ftplib_err(interp, "error_reply", resp));
    }
    if !resp[3..].starts_with(" \"") {
        return Ok(String::new());
    }
    let mut dirname = String::new();
    let chars: Vec<char> = resp[5..].chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c == '"' {
            if i >= chars.len() || chars[i] != '"' {
                break;
            }
            i += 1;
        }
        dirname.push(c);
    }
    Ok(dirname)
}

// ── Shared FTP Core Helpers (used by FTP and FTP_TLS) ────────────────────────

#[allow(clippy::too_many_arguments)]
fn ftp_init_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    host: &str,
    user: &str,
    passwd: &str,
    acct: &str,
    timeout: Value,
    source_address: Value,
    encoding: &str,
) -> Result<()> {
    interp.assign_attr(self_val.clone(), "encoding", Value::string(encoding))?;
    interp.assign_attr(self_val.clone(), "source_address", source_address)?;
    interp.assign_attr(self_val.clone(), "timeout", timeout)?;
    if !host.is_empty() {
        call_method(interp, self_val, "connect", &[Value::string(host)])?;
        if !user.is_empty() {
            call_method(
                interp,
                self_val,
                "login",
                &[
                    Value::string(user),
                    Value::string(passwd),
                    Value::string(acct),
                ],
            )?;
        }
    }
    Ok(())
}

fn ftp_login_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    user: &str,
    passwd: &str,
    acct: &str,
) -> Result<Value> {
    let user_eff = if user.is_empty() { "anonymous" } else { user };
    let passwd_eff = if user_eff == "anonymous" && (passwd.is_empty() || passwd == "-") {
        format!("{passwd}anonymous@")
    } else {
        passwd.to_string()
    };

    let mut resp = call_method(
        interp,
        self_val,
        "sendcmd",
        &[Value::string(format!("USER {user_eff}"))],
    )?;
    if resp.as_str().is_some_and(|s| s.starts_with('3')) {
        resp = call_method(
            interp,
            self_val,
            "sendcmd",
            &[Value::string(format!("PASS {passwd_eff}"))],
        )?;
    }
    if resp.as_str().is_some_and(|s| s.starts_with('3')) {
        resp = call_method(
            interp,
            self_val,
            "sendcmd",
            &[Value::string(format!("ACCT {acct}"))],
        )?;
    }
    if !resp.as_str().is_some_and(|s| s.starts_with('2')) {
        let msg = value_to_str(interp, &resp)?;
        return Err(raise_ftplib_err(interp, "error_reply", &msg));
    }
    Ok(resp)
}

fn ftp_ntransfercmd_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    cmd: &str,
    rest: Option<Value>,
) -> Result<(Value, Value)> {
    let passive = {
        let v = interp.get_attr(self_val, "passiveserver")?;
        interp.truthy_value(&v)?
    };
    let timeout = interp.get_attr(self_val, "timeout")?;
    let source_address = interp.get_attr(self_val, "source_address")?;
    let sock_mod = interp.load_module("socket")?;

    let (conn, resp_str) = if passive {
        let hp = call_method(interp, self_val, "makepasv", &[])?;
        let create_conn = interp.get_attr(&sock_mod, "create_connection")?;
        let conn = call_kw(
            interp,
            create_conn,
            &[hp, timeout],
            &[("source_address", source_address)],
        )?;
        let res: Result<String> = (|| {
            if let Some(ref r) = rest {
                let rs = value_to_str(interp, r)?;
                call_method(
                    interp,
                    self_val,
                    "sendcmd",
                    &[Value::string(format!("REST {rs}"))],
                )?;
            }
            let mut resp = call_method(interp, self_val, "sendcmd", &[Value::string(cmd)])?;
            if resp.as_str().is_some_and(|s| s.starts_with('2')) {
                resp = call_method(interp, self_val, "getresp", &[])?;
            }
            let rs = value_to_str(interp, &resp)?;
            if !rs.starts_with('1') {
                return Err(raise_ftplib_err(interp, "error_reply", &rs));
            }
            Ok(rs)
        })();
        match res {
            Ok(rs) => (conn, rs),
            Err(e) => {
                let _ = call_method(interp, &conn, "close", &[]);
                return Err(e);
            }
        }
    } else {
        let listen_sock = call_method(interp, self_val, "makeport", &[])?;
        let res: Result<(Value, String)> = (|| {
            if let Some(ref r) = rest {
                let rs = value_to_str(interp, r)?;
                call_method(
                    interp,
                    self_val,
                    "sendcmd",
                    &[Value::string(format!("REST {rs}"))],
                )?;
            }
            let mut resp = call_method(interp, self_val, "sendcmd", &[Value::string(cmd)])?;
            if resp.as_str().is_some_and(|s| s.starts_with('2')) {
                resp = call_method(interp, self_val, "getresp", &[])?;
            }
            let rs = value_to_str(interp, &resp)?;
            if !rs.starts_with('1') {
                return Err(raise_ftplib_err(interp, "error_reply", &rs));
            }
            let accepted = call_method(interp, &listen_sock, "accept", &[])?;
            let conn = py_getitem(interp, &accepted, Value::int(0))?;
            let gdt = global_default_timeout(interp)?;
            if !is_same_object(interp, &timeout, &gdt)? {
                call_method(interp, &conn, "settimeout", &[timeout])?;
            }
            Ok((conn, rs))
        })();
        let _ = call_method(interp, &listen_sock, "close", &[]);
        res?
    };

    let size = if resp_str.starts_with("150") {
        parse150_impl(interp, &resp_str)?
    } else {
        Value::none()
    };
    Ok((conn, size))
}

// ── MLSD lazy iterator ───────────────────────────────────────────────────────

struct MlsdIterState {
    ftp: Value,
    path: String,
    facts: Vec<String>,
    items: Option<Vec<Value>>,
    index: usize,
}

fn parse_mlsd_line(line: &str) -> Value {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let (facts_found, name) = match trimmed.split_once(' ') {
        Some((f, n)) => (f, n),
        None => (trimmed, ""),
    };
    let mut dict = PyDict::default();
    let facts_body = facts_found.strip_suffix(';').unwrap_or(facts_found);
    if !facts_body.is_empty() {
        for fact in facts_body.split(';') {
            if let Some((k, v)) = fact.split_once('=') {
                dict.insert(PyKey::str_from(k.to_ascii_lowercase()), Value::string(v));
            } else if !fact.is_empty() {
                dict.insert(
                    PyKey::str_from(fact.to_ascii_lowercase()),
                    Value::string(""),
                );
            }
        }
    }
    Value::tuple(vec![Value::string(name), Value::dict(dict)])
}

// ── Post-load initialization ─────────────────────────────────────────────────

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let err_cls = module
        .borrow()
        .attrs
        .get("Error")
        .cloned()
        .ok_or_else(|| PyError::runtime("ftplib.Error missing".into()))?;
    let os_err = Value::py_class(lookup_exc_class("OSError").expect("OSError must exist"));
    let eof_err = Value::py_class(lookup_exc_class("EOFError").expect("EOFError must exist"));
    let ssl_mod = interp.load_module("ssl")?;
    let ssl_err = interp.get_attr(&ssl_mod, "SSLError")?;
    module.borrow_mut().insert_attr(
        "all_errors".to_string(),
        Value::tuple(vec![err_cls, os_err, eof_err, ssl_err]),
    );

    // Alias `FTP.debug = FTP.set_debuglevel`.
    let ftp_opt = module.borrow().attrs.get("FTP").cloned();
    if let Some(ftp_val) = ftp_opt
        && let ValueKind::PyClass(ftp_rc) = ftp_val.kind()
    {
        let set_dbg = ftp_rc.borrow().attrs.get("set_debuglevel").cloned();
        if let Some(f) = set_dbg {
            ftp_rc.borrow_mut().attrs.insert("debug".to_string(), f);
        }
    }

    Ok(())
}

// ── Native module definition ─────────────────────────────────────────────────

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("FTP"),
            Value::string("error_reply"),
            Value::string("error_temp"),
            Value::string("error_perm"),
            Value::string("error_proto"),
            Value::string("all_errors"),
            Value::string("FTP_TLS"),
        ]),
        "MSG_OOB" => Value::int(MSG_OOB_VAL),
        "FTP_PORT" => Value::int(FTP_PORT_VAL),
        "MAXLINE" => Value::int(MAXLINE_VAL),
        "CRLF" => Value::string(CRLF_STR),
        "B_CRLF" => Value::bytes(B_CRLF_BYTES.to_vec()),
    }

    // ── Exception hierarchy ──────────────────────────────────────────────────

    /// Base exception raised by ftplib.
    exception Error;
    /// Unexpected [123]xx reply.
    exception error_reply(Error);
    /// Temporary 4xx error.
    exception error_temp(Error);
    /// Permanent 5xx error.
    exception error_perm(Error);
    /// Protocol error (reply does not begin with [1-5]).
    exception error_proto(Error);

    // ── Internal MLSD Iterator ───────────────────────────────────────────────

    #[native_state(MlsdIterState)]
    class _MLSDIterator {
        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let need_fetch = with_state(self_val.0, |st| Ok(st.items.is_none()))?;
            if need_fetch {
                let (ftp, path, facts) = with_state(self_val.0, |st| {
                    Ok((st.ftp.clone(), st.path.clone(), st.facts.clone()))
                })?;
                if !facts.is_empty() {
                    let opts = format!("OPTS MLST {};", facts.join(";"));
                    call_method(_interp, &ftp, "sendcmd", &[Value::string(opts)])?;
                }
                let cmd = if path.is_empty() {
                    "MLSD".to_string()
                } else {
                    format!("MLSD {path}")
                };
                let lines = Value::list(Vec::new());
                let append_fn = _interp.get_attr(&lines, "append")?;
                call_method(_interp, &ftp, "retrlines", &[Value::string(cmd), append_fn])?;
                let raw_lines = _interp.collect_iterable(&lines)?;
                let mut parsed = Vec::with_capacity(raw_lines.len());
                for lv in raw_lines {
                    let s = value_to_str(_interp, &lv)?;
                    parsed.push(parse_mlsd_line(&s));
                }
                with_state_mut(self_val.0, |st| {
                    st.items = Some(parsed);
                    Ok(())
                })?;
            }
            with_state_mut(self_val.0, |st| {
                let items = st.items.as_ref().expect("populated");
                if st.index < items.len() {
                    let v = items[st.index].clone();
                    st.index += 1;
                    Ok(v)
                } else {
                    Err(PyError::named("StopIteration", ""))
                }
            })
        }
    }

    // ── FTP ──────────────────────────────────────────────────────────────────

    class FTP {
        constants {
            "debugging" => Value::int(0),
            "host" => Value::string(""),
            "port" => Value::int(FTP_PORT_VAL),
            "maxline" => Value::int(MAXLINE_VAL),
            "sock" => Value::none(),
            "file" => Value::none(),
            "welcome" => Value::none(),
            "passiveserver" => Value::bool_(true),
            "trust_server_pasv_ipv4_address" => Value::bool_(false),
        }

        #[allow(clippy::too_many_arguments)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] host: PyStr<'_>,
            #[default("".into())] user: PyStr<'_>,
            #[default("".into())] passwd: PyStr<'_>,
            #[default("".into())] acct: PyStr<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] source_address: OptValueRef<'_>,
            #[keyword_only]
            #[default("utf-8".into())]
            encoding: PyStr<'_>,
        ) -> Result<Value> {
            let timeout_val = match timeout.as_value() {
                Some(v) => v.clone(),
                None => global_default_timeout(_interp)?,
            };
            let src_val = source_address
                .as_value()
                .cloned()
                .unwrap_or_else(Value::none);
            ftp_init_impl(
                _interp,
                self_val.0,
                &host.0,
                &user.0,
                &passwd.0,
                &acct.0,
                timeout_val,
                src_val,
                &encoding.0,
            )?;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            let sock = _interp.get_attr(self_val.0, "sock")?;
            if !sock.is_none() {
                let quit_res = call_method(_interp, self_val.0, "quit", &[]);
                let close_needed = !_interp.get_attr(self_val.0, "sock")?.is_none();
                if close_needed {
                    let _ = call_method(_interp, self_val.0, "close", &[]);
                }
                if let Err(err) = quit_res {
                    let os_cls =
                        Value::py_class(lookup_exc_class("OSError").expect("OSError"));
                    let eof_cls =
                        Value::py_class(lookup_exc_class("EOFError").expect("EOFError"));
                    let ignored = match err.kind() {
                        PyErrorKind::Raised(exc) => {
                            is_instance(_interp, exc, os_cls).unwrap_or(false)
                                || is_instance(_interp, exc, eof_cls).unwrap_or(false)
                        }
                        PyErrorKind::OsError { .. } => true,
                        PyErrorKind::Named(name, _) => matches!(
                            name.as_ref(),
                            "OSError"
                                | "EOFError"
                                | "ConnectionError"
                                | "BrokenPipeError"
                                | "ConnectionResetError"
                                | "TimeoutError"
                        ),
                        _ => false,
                    };
                    if !ignored {
                        return Err(err);
                    }
                }
            }
            Ok(Value::none())
        }

        fn connect(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] host: PyStr<'_>,
            #[default(0_i64.into())] port: PyIndex,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] source_address: OptValueRef<'_>,
        ) -> Result<Value> {
            if !host.0.is_empty() {
                _interp.assign_attr(self_val.0.clone(), "host", Value::string(&*host.0))?;
            }
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            if port_i64 > 0 {
                _interp.assign_attr(self_val.0.clone(), "port", Value::int(port_i64))?;
            }
            if let Some(t) = timeout.as_value()
                && t.as_int() != Some(-999)
            {
                _interp.assign_attr(self_val.0.clone(), "timeout", t.clone())?;
            }
            let cur_timeout = _interp.get_attr(self_val.0, "timeout")?;
            if !cur_timeout.is_none() && !_interp.truthy_value(&cur_timeout)? {
                return Err(PyError::named(
                    "ValueError",
                    "Non-blocking socket (timeout=0) is not supported",
                ));
            }
            if let Some(sa) = source_address.as_value()
                && !sa.is_none()
            {
                _interp.assign_attr(self_val.0.clone(), "source_address", sa.clone())?;
            }

            let cur_host = get_str_attr(_interp, self_val.0, "host", "")?;
            let cur_port = get_int_attr(_interp, self_val.0, "port", FTP_PORT_VAL)?;
            let cur_src = _interp
                .get_attr(self_val.0, "source_address")
                .unwrap_or_else(|_| Value::none());
            let encoding = get_str_attr(_interp, self_val.0, "encoding", "utf-8")?;

            let sock_mod = _interp.load_module("socket")?;
            let create_conn = _interp.get_attr(&sock_mod, "create_connection")?;
            let addr = Value::tuple(vec![Value::string(cur_host), Value::int(cur_port)]);
            let sock = call_kw(
                _interp,
                create_conn,
                &[addr, cur_timeout],
                &[("source_address", cur_src)],
            )?;
            let af = _interp.get_attr(&sock, "family")?;
            let file = call_method_kw(
                _interp,
                &sock,
                "makefile",
                &[Value::string("r")],
                &[("encoding", Value::string(encoding))],
            )?;
            _interp.assign_attr(self_val.0.clone(), "sock", sock)?;
            _interp.assign_attr(self_val.0.clone(), "af", af)?;
            _interp.assign_attr(self_val.0.clone(), "file", file)?;
            let welcome = call_method(_interp, self_val.0, "getresp", &[])?;
            _interp.assign_attr(self_val.0.clone(), "welcome", welcome.clone())?;
            Ok(welcome)
        }

        fn getwelcome(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let welcome = _interp.get_attr(self_val.0, "welcome")?;
            let dbg = get_int_attr(_interp, self_val.0, "debugging", 0)?;
            if dbg != 0 {
                let s = call_method(_interp, self_val.0, "sanitize", std::slice::from_ref(&welcome))?;
                print_debug_pair(_interp, "*welcome*", &s)?;
            }
            Ok(welcome)
        }

        fn set_debuglevel(
            #[positional_only] self_val: PyValueRef<'_>,
            level: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "debugging", level.0)?;
            Ok(Value::none())
        }

        fn set_pasv(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "passiveserver", val.0)?;
            Ok(Value::none())
        }

        fn sanitize(#[positional_only] _self: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            Ok(Value::string(sanitize_str(_interp, &s.0)?))
        }

        fn putline(
            #[positional_only] self_val: PyValueRef<'_>,
            line: PyStr<'_>,
        ) -> Result<Value> {
            if line.0.contains('\r') || line.0.contains('\n') {
                return Err(PyError::named(
                    "ValueError",
                    "an illegal newline character should not be contained",
                ));
            }
            let full = format!("{}{CRLF_STR}", line.0);
            let dbg = get_int_attr(_interp, self_val.0, "debugging", 0)?;
            if dbg > 1 {
                let s = call_method(_interp, self_val.0, "sanitize", &[Value::string(&full)])?;
                print_debug_pair(_interp, "*put*", &s)?;
            }
            let encoding = get_str_attr(_interp, self_val.0, "encoding", "utf-8")?;
            let encoded = call_method(
                _interp,
                &Value::string(full),
                "encode",
                &[Value::string(encoding)],
            )?;
            let sock = _interp.get_attr(self_val.0, "sock")?;
            call_method(_interp, &sock, "sendall", &[encoded])?;
            Ok(Value::none())
        }

        fn putcmd(
            #[positional_only] self_val: PyValueRef<'_>,
            line: PyStr<'_>,
        ) -> Result<Value> {
            let dbg = get_int_attr(_interp, self_val.0, "debugging", 0)?;
            if dbg != 0 {
                let s = call_method(_interp, self_val.0, "sanitize", &[Value::string(&*line.0)])?;
                print_debug_pair(_interp, "*cmd*", &s)?;
            }
            call_method(_interp, self_val.0, "putline", &[Value::string(&*line.0)])?;
            Ok(Value::none())
        }

        fn getline(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let maxline = get_int_attr(_interp, self_val.0, "maxline", MAXLINE_VAL)?;
            let file = _interp.get_attr(self_val.0, "file")?;
            let line_val = call_method(_interp, &file, "readline", &[Value::int(maxline + 1)])?;
            let line = value_to_str(_interp, &line_val)?;
            if (line.chars().count() as i64) > maxline {
                return Err(raise_ftplib_err(
                    _interp,
                    "Error",
                    &format!("got more than {maxline} bytes"),
                ));
            }
            let dbg = get_int_attr(_interp, self_val.0, "debugging", 0)?;
            if dbg > 1 {
                let s = call_method(_interp, self_val.0, "sanitize", &[Value::string(&line)])?;
                print_debug_pair(_interp, "*get*", &s)?;
            }
            if line.is_empty() {
                return Err(PyError::named("EOFError", ""));
            }
            let stripped = if let Some(s) = line.strip_suffix("\r\n") {
                s
            } else if line.ends_with('\r') || line.ends_with('\n') {
                &line[..line.len() - 1]
            } else {
                &line
            };
            Ok(Value::string(stripped))
        }

        fn getmultiline(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let first_val = call_method(_interp, self_val.0, "getline", &[])?;
            let mut line = value_to_str(_interp, &first_val)?;
            if line.len() >= 4 && &line.as_bytes()[3..4] == b"-" {
                let code = line[..3].to_string();
                loop {
                    let next_val = call_method(_interp, self_val.0, "getline", &[])?;
                    let nextline = value_to_str(_interp, &next_val)?;
                    line.push('\n');
                    line.push_str(&nextline);
                    if nextline.starts_with(&code)
                        && (nextline.len() < 4 || nextline.as_bytes()[3] != b'-')
                    {
                        break;
                    }
                }
            }
            Ok(Value::string(line))
        }

        fn getresp(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let resp_val = call_method(_interp, self_val.0, "getmultiline", &[])?;
            let resp = value_to_str(_interp, &resp_val)?;
            let dbg = get_int_attr(_interp, self_val.0, "debugging", 0)?;
            if dbg != 0 {
                let s = call_method(_interp, self_val.0, "sanitize", &[Value::string(&resp)])?;
                print_debug_pair(_interp, "*resp*", &s)?;
            }
            let lastresp: String = resp.chars().take(3).collect();
            _interp.assign_attr(self_val.0.clone(), "lastresp", Value::string(lastresp))?;
            match resp.as_bytes().first().copied() {
                Some(b'1' | b'2' | b'3') => Ok(Value::string(resp)),
                Some(b'4') => Err(raise_ftplib_err(_interp, "error_temp", &resp)),
                Some(b'5') => Err(raise_ftplib_err(_interp, "error_perm", &resp)),
                _ => Err(raise_ftplib_err(_interp, "error_proto", &resp)),
            }
        }

        fn voidresp(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let resp_val = call_method(_interp, self_val.0, "getresp", &[])?;
            let resp = value_to_str(_interp, &resp_val)?;
            if !resp.starts_with('2') {
                return Err(raise_ftplib_err(_interp, "error_reply", &resp));
            }
            Ok(Value::string(resp))
        }

        fn abort(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let line = Value::bytes(b"ABOR\r\n".to_vec());
            let dbg = get_int_attr(_interp, self_val.0, "debugging", 0)?;
            if dbg > 1 {
                let s = call_method(_interp, self_val.0, "sanitize", std::slice::from_ref(&line))?;
                print_debug_pair(_interp, "*put urgent*", &s)?;
            }
            let sock = _interp.get_attr(self_val.0, "sock")?;
            call_method(_interp, &sock, "sendall", &[line, Value::int(MSG_OOB_VAL)])?;
            let resp_val = call_method(_interp, self_val.0, "getmultiline", &[])?;
            let resp = value_to_str(_interp, &resp_val)?;
            let code: String = resp.chars().take(3).collect();
            if !matches!(code.as_str(), "426" | "225" | "226") {
                return Err(raise_ftplib_err(_interp, "error_proto", &resp));
            }
            Ok(Value::string(resp))
        }

        fn sendcmd(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "putcmd", &[Value::string(&*cmd.0)])?;
            call_method(_interp, self_val.0, "getresp", &[])
        }

        fn voidcmd(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "putcmd", &[Value::string(&*cmd.0)])?;
            call_method(_interp, self_val.0, "voidresp", &[])
        }

        fn sendport(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyStr<'_>,
            port: PyIndex,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let mut parts: Vec<String> = host.0.split('.').map(str::to_string).collect();
            parts.push((port_i64 / 256).to_string());
            parts.push((port_i64 % 256).to_string());
            let cmd = format!("PORT {}", parts.join(","));
            call_method(_interp, self_val.0, "voidcmd", &[Value::string(cmd)])
        }

        fn sendeprt(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyStr<'_>,
            port: PyIndex,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let sock_mod = _interp.load_module("socket")?;
            let af_inet = _interp.get_attr(&sock_mod, "AF_INET")?;
            let af_inet6 = _interp.get_attr(&sock_mod, "AF_INET6")?;
            let self_af = _interp.get_attr(self_val.0, "af")?;
            let af_num = if _interp
                .eval_binary(self_af.clone(), crate::ast::BinaryOp::Eq, af_inet)?
                .as_bool()
            {
                1
            } else if _interp
                .eval_binary(self_af, crate::ast::BinaryOp::Eq, af_inet6)?
                .as_bool()
            {
                2
            } else {
                return Err(raise_ftplib_err(
                    _interp,
                    "error_proto",
                    "unsupported address family",
                ));
            };
            let cmd = format!("EPRT |{af_num}|{}|{port_i64}|", host.0);
            call_method(_interp, self_val.0, "voidcmd", &[Value::string(cmd)])
        }

        fn makeport(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let sock_mod = _interp.load_module("socket")?;
            let create_server = _interp.get_attr(&sock_mod, "create_server")?;
            let self_af = _interp.get_attr(self_val.0, "af")?;
            let bind_addr = Value::tuple(vec![Value::string(""), Value::int(0)]);
            let sock = call_kw(
                _interp,
                create_server,
                &[bind_addr],
                &[("family", self_af.clone()), ("backlog", Value::int(1))],
            )?;
            let sname = call_method(_interp, &sock, "getsockname", &[])?;
            let port = py_getitem(_interp, &sname, Value::int(1))?;
            let ctrl_sock = _interp.get_attr(self_val.0, "sock")?;
            let ctrl_name = call_method(_interp, &ctrl_sock, "getsockname", &[])?;
            let host = py_getitem(_interp, &ctrl_name, Value::int(0))?;

            let af_inet = _interp.get_attr(&sock_mod, "AF_INET")?;
            let is_v4 = _interp
                .eval_binary(self_af, crate::ast::BinaryOp::Eq, af_inet)?
                .as_bool();
            let send_res = if is_v4 {
                call_method(_interp, self_val.0, "sendport", &[host, port])
            } else {
                call_method(_interp, self_val.0, "sendeprt", &[host, port])
            };
            if let Err(e) = send_res {
                let _ = call_method(_interp, &sock, "close", &[]);
                return Err(e);
            }
            let timeout = _interp.get_attr(self_val.0, "timeout")?;
            let gdt = global_default_timeout(_interp)?;
            if !is_same_object(_interp, &timeout, &gdt)? {
                call_method(_interp, &sock, "settimeout", &[timeout])?;
            }
            Ok(sock)
        }

        fn makepasv(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let sock_mod = _interp.load_module("socket")?;
            let af_inet = _interp.get_attr(&sock_mod, "AF_INET")?;
            let self_af = _interp.get_attr(self_val.0, "af")?;
            let ctrl_sock = _interp.get_attr(self_val.0, "sock")?;
            let is_v4 = _interp
                .eval_binary(self_af, crate::ast::BinaryOp::Eq, af_inet)?
                .as_bool();
            if is_v4 {
                let resp = call_method(_interp, self_val.0, "sendcmd", &[Value::string("PASV")])?;
                let resp_str = value_to_str(_interp, &resp)?;
                let (untrusted_host, port) = parse227_impl(_interp, &resp_str)?;
                let trust = {
                    let t = _interp.get_attr(self_val.0, "trust_server_pasv_ipv4_address")?;
                    _interp.truthy_value(&t)?
                };
                let host = if trust {
                    Value::string(untrusted_host)
                } else {
                    let peer = call_method(_interp, &ctrl_sock, "getpeername", &[])?;
                    py_getitem(_interp, &peer, Value::int(0))?
                };
                Ok(Value::tuple(vec![host, Value::int(port)]))
            } else {
                let resp = call_method(_interp, self_val.0, "sendcmd", &[Value::string("EPSV")])?;
                let resp_str = value_to_str(_interp, &resp)?;
                let peer = call_method(_interp, &ctrl_sock, "getpeername", &[])?;
                let (host, port) = parse229_impl(_interp, &resp_str, &peer)?;
                Ok(Value::tuple(vec![host, Value::int(port)]))
            }
        }

        fn ntransfercmd(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
            #[default(OptionalArg::Omitted)] rest: OptValueRef<'_>,
        ) -> Result<Value> {
            let rest_opt = rest.as_value().filter(|v| !v.is_none()).cloned();
            let (conn, size) = ftp_ntransfercmd_impl(_interp, self_val.0, &cmd.0, rest_opt)?;
            Ok(Value::tuple(vec![conn, size]))
        }

        fn transfercmd(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
            #[default(OptionalArg::Omitted)] rest: OptValueRef<'_>,
        ) -> Result<Value> {
            let rest_val = rest.as_value().cloned().unwrap_or_else(Value::none);
            let pair = call_method(
                _interp,
                self_val.0,
                "ntransfercmd",
                &[Value::string(&*cmd.0), rest_val],
            )?;
            py_getitem(_interp, &pair, Value::int(0))
        }

        fn login(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] user: PyStr<'_>,
            #[default("".into())] passwd: PyStr<'_>,
            #[default("".into())] acct: PyStr<'_>,
        ) -> Result<Value> {
            ftp_login_impl(_interp, self_val.0, &user.0, &passwd.0, &acct.0)
        }

        fn retrbinary(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
            callback: PyValue,
            #[default(8192_i64.into())] blocksize: PyIndex,
            #[default(OptionalArg::Omitted)] rest: OptValueRef<'_>,
        ) -> Result<Value> {
            let bs = blocksize.expect_i64(FN_NAME, "blocksize")?;
            call_method(_interp, self_val.0, "voidcmd", &[Value::string("TYPE I")])?;
            let rest_val = rest.as_value().cloned().unwrap_or_else(Value::none);
            let conn = call_method(
                _interp,
                self_val.0,
                "transfercmd",
                &[Value::string(&*cmd.0), rest_val],
            )?;
            let res: Result<()> = (|| {
                loop {
                    let data = call_method(_interp, &conn, "recv", &[Value::int(bs)])?;
                    if !_interp.truthy_value(&data)? {
                        break;
                    }
                    call_pos(_interp, callback.0.clone(), &[data])?;
                }
                unwrap_and_close_ssl_conn(_interp, &conn)?;
                Ok(())
            })();
            let _ = call_method(_interp, &conn, "close", &[]);
            res?;
            call_method(_interp, self_val.0, "voidresp", &[])
        }

        fn retrlines(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
            #[default(OptionalArg::Omitted)] callback: OptValueRef<'_>,
        ) -> Result<Value> {
            let cb = match callback.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => ftplib_attr(_interp, "print_line")?,
            };
            call_method(_interp, self_val.0, "sendcmd", &[Value::string("TYPE A")])?;
            let encoding = get_str_attr(_interp, self_val.0, "encoding", "utf-8")?;
            let maxline = get_int_attr(_interp, self_val.0, "maxline", MAXLINE_VAL)?;
            let dbg = get_int_attr(_interp, self_val.0, "debugging", 0)?;

            let conn = call_method(_interp, self_val.0, "transfercmd", &[Value::string(&*cmd.0)])?;
            let fp = match call_method_kw(
                _interp,
                &conn,
                "makefile",
                &[Value::string("r")],
                &[("encoding", Value::string(encoding))],
            ) {
                Ok(f) => f,
                Err(e) => {
                    let _ = call_method(_interp, &conn, "close", &[]);
                    return Err(e);
                }
            };
            let res: Result<()> = (|| {
                loop {
                    let line_val =
                        call_method(_interp, &fp, "readline", &[Value::int(maxline + 1)])?;
                    let line = value_to_str(_interp, &line_val)?;
                    if (line.chars().count() as i64) > maxline {
                        return Err(raise_ftplib_err(
                            _interp,
                            "Error",
                            &format!("got more than {maxline} bytes"),
                        ));
                    }
                    if dbg > 2 {
                        let r = render_value_repr(_interp, &line_val)?;
                        print_debug_pair(_interp, "*retr*", &Value::string(r))?;
                    }
                    if line.is_empty() {
                        break;
                    }
                    let stripped = if let Some(s) = line.strip_suffix("\r\n") {
                        s
                    } else if let Some(s) = line.strip_suffix('\n') {
                        s
                    } else {
                        &line
                    };
                    call_pos(_interp, cb.clone(), &[Value::string(stripped)])?;
                }
                unwrap_and_close_ssl_conn(_interp, &conn)?;
                Ok(())
            })();
            let _ = call_method(_interp, &fp, "close", &[]);
            let _ = call_method(_interp, &conn, "close", &[]);
            res?;
            call_method(_interp, self_val.0, "voidresp", &[])
        }

        fn storbinary(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
            fp: PyValue,
            #[default(8192_i64.into())] blocksize: PyIndex,
            #[default(OptionalArg::Omitted)] callback: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] rest: OptValueRef<'_>,
        ) -> Result<Value> {
            let bs = blocksize.expect_i64(FN_NAME, "blocksize")?;
            let cb_opt = callback.as_value().filter(|v| !v.is_none()).cloned();
            let rest_val = rest.as_value().cloned().unwrap_or_else(Value::none);
            call_method(_interp, self_val.0, "voidcmd", &[Value::string("TYPE I")])?;
            let conn = call_method(
                _interp,
                self_val.0,
                "transfercmd",
                &[Value::string(&*cmd.0), rest_val],
            )?;
            let res: Result<()> = (|| {
                loop {
                    let buf = call_method(_interp, &fp.0, "read", &[Value::int(bs)])?;
                    if !_interp.truthy_value(&buf)? {
                        break;
                    }
                    call_method(_interp, &conn, "sendall", std::slice::from_ref(&buf))?;
                    if let Some(ref cb) = cb_opt {
                        call_pos(_interp, cb.clone(), &[buf])?;
                    }
                }
                unwrap_and_close_ssl_conn(_interp, &conn)?;
                Ok(())
            })();
            let _ = call_method(_interp, &conn, "close", &[]);
            res?;
            call_method(_interp, self_val.0, "voidresp", &[])
        }

        fn storlines(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
            fp: PyValue,
            #[default(OptionalArg::Omitted)] callback: OptValueRef<'_>,
        ) -> Result<Value> {
            let cb_opt = callback.as_value().filter(|v| !v.is_none()).cloned();
            let maxline = get_int_attr(_interp, self_val.0, "maxline", MAXLINE_VAL)?;
            call_method(_interp, self_val.0, "voidcmd", &[Value::string("TYPE A")])?;
            let conn = call_method(_interp, self_val.0, "transfercmd", &[Value::string(&*cmd.0)])?;
            let res: Result<()> = (|| {
                loop {
                    let buf_val =
                        call_method(_interp, &fp.0, "readline", &[Value::int(maxline + 1)])?;
                    let raw = match buf_val.kind() {
                        ValueKind::Bytes(b) => b.as_ref().clone(),
                        _ => {
                            if let Some(ba) = pyrust_builtins::bytearray::as_bytearray_snapshot(&buf_val) {
                                ba
                            } else {
                                return Err(PyError::named(
                                    "TypeError",
                                    format!(
                                        "a bytes-like object is required, not '{}'",
                                        value_type_name_str(&buf_val)
                                    ),
                                ));
                            }
                        }
                    };
                    if (raw.len() as i64) > maxline {
                        return Err(raise_ftplib_err(
                            _interp,
                            "Error",
                            &format!("got more than {maxline} bytes"),
                        ));
                    }
                    if raw.is_empty() {
                        break;
                    }
                    let mut out = raw;
                    if !out.ends_with(B_CRLF_BYTES) {
                        if matches!(out.last(), Some(b'\r' | b'\n')) {
                            out.pop();
                        }
                        out.extend_from_slice(B_CRLF_BYTES);
                    }
                    let send_val = Value::bytes(out);
                    call_method(_interp, &conn, "sendall", std::slice::from_ref(&send_val))?;
                    if let Some(ref cb) = cb_opt {
                        call_pos(_interp, cb.clone(), &[send_val])?;
                    }
                }
                unwrap_and_close_ssl_conn(_interp, &conn)?;
                Ok(())
            })();
            let _ = call_method(_interp, &conn, "close", &[]);
            res?;
            call_method(_interp, self_val.0, "voidresp", &[])
        }

        fn acct(
            #[positional_only] self_val: PyValueRef<'_>,
            password: PyStr<'_>,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "voidcmd",
                &[Value::string(format!("ACCT {}", password.0))],
            )
        }

        fn nlst(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let mut cmd = String::from("NLST");
            for arg in args {
                let s = value_to_str(_interp, arg)?;
                cmd.push(' ');
                cmd.push_str(&s);
            }
            let files = Value::list(Vec::new());
            let append_fn = _interp.get_attr(&files, "append")?;
            call_method(_interp, self_val.0, "retrlines", &[Value::string(cmd), append_fn])?;
            Ok(files)
        }

        fn dir(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let mut cmd = String::from("LIST");
            let (path_args, func) = if let Some((last, rest)) = args.split_last() {
                if last.as_str().is_none() {
                    (rest, last.clone())
                } else {
                    (args, Value::none())
                }
            } else {
                (args, Value::none())
            };
            for arg in path_args {
                let s = value_to_str(_interp, arg)?;
                if !s.is_empty() {
                    cmd.push(' ');
                    cmd.push_str(&s);
                }
            }
            call_method(_interp, self_val.0, "retrlines", &[Value::string(cmd), func])?;
            Ok(Value::none())
        }

        fn mlsd(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] path: PyStr<'_>,
            #[default(OptionalArg::Omitted)] facts: OptValueRef<'_>,
        ) -> Result<Value> {
            let mut fact_list = Vec::new();
            if let Some(fv) = facts.as_value() {
                let items = _interp.collect_iterable(fv)?;
                for item in items {
                    fact_list.push(value_to_str(_interp, &item)?);
                }
            }
            let iter_cls = ftplib_attr(_interp, "_MLSDIterator")?;
            let ValueKind::PyClass(cls_rc) = iter_cls.kind() else {
                return Err(PyError::runtime("ftplib._MLSDIterator is not a class".into()));
            };
            let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class: Rc::clone(cls_rc),
                attrs: InstanceAttrs::new(),
            })));
            set_native_state(
                &inst,
                MlsdIterState {
                    ftp: self_val.0.clone(),
                    path: path.0.to_string(),
                    facts: fact_list,
                    items: None,
                    index: 0,
                },
            )?;
            Ok(inst)
        }

        fn rename(
            #[positional_only] self_val: PyValueRef<'_>,
            fromname: PyStr<'_>,
            toname: PyStr<'_>,
        ) -> Result<Value> {
            let resp = call_method(
                _interp,
                self_val.0,
                "sendcmd",
                &[Value::string(format!("RNFR {}", fromname.0))],
            )?;
            let rs = value_to_str(_interp, &resp)?;
            if !rs.starts_with('3') {
                return Err(raise_ftplib_err(_interp, "error_reply", &rs));
            }
            call_method(
                _interp,
                self_val.0,
                "voidcmd",
                &[Value::string(format!("RNTO {}", toname.0))],
            )
        }

        fn delete(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            let resp = call_method(
                _interp,
                self_val.0,
                "sendcmd",
                &[Value::string(format!("DELE {}", filename.0))],
            )?;
            let rs = value_to_str(_interp, &resp)?;
            if rs.starts_with("250") || rs.starts_with("200") {
                Ok(Value::string(rs))
            } else {
                Err(raise_ftplib_err(_interp, "error_reply", &rs))
            }
        }

        fn cwd(
            #[positional_only] self_val: PyValueRef<'_>,
            dirname: PyStr<'_>,
        ) -> Result<Value> {
            let mut dir = dirname.0.to_string();
            if dir == ".." {
                match call_method(_interp, self_val.0, "voidcmd", &[Value::string("CDUP")]) {
                    Ok(resp) => return Ok(resp),
                    Err(err) => {
                        let perm_cls = ftplib_attr(_interp, "error_perm")?;
                        let is_500 = match err.kind() {
                            PyErrorKind::Raised(exc)
                                if is_instance(_interp, exc, perm_cls).unwrap_or(false) =>
                            {
                                _interp
                                    .get_attr(exc, "args")
                                    .ok()
                                    .and_then(|a| py_getitem(_interp, &a, Value::int(0)).ok())
                                    .and_then(|m| m.as_str().map(|s| s.starts_with("500")))
                                    .unwrap_or(false)
                            }
                            _ => false,
                        };
                        if !is_500 {
                            return Err(err);
                        }
                    }
                }
            } else if dir.is_empty() {
                dir = ".".to_string();
            }
            call_method(
                _interp,
                self_val.0,
                "voidcmd",
                &[Value::string(format!("CWD {dir}"))],
            )
        }

        fn size(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            let resp = call_method(
                _interp,
                self_val.0,
                "sendcmd",
                &[Value::string(format!("SIZE {}", filename.0))],
            )?;
            let rs = value_to_str(_interp, &resp)?;
            if let Some(rest) = rs.strip_prefix("213") {
                let s = rest.trim();
                let n: i64 = s.parse().map_err(|_| {
                    PyError::named("ValueError", format!("invalid literal for int(): {s:?}"))
                })?;
                return Ok(Value::int(n));
            }
            Ok(Value::none())
        }

        fn mkd(
            #[positional_only] self_val: PyValueRef<'_>,
            dirname: PyStr<'_>,
        ) -> Result<Value> {
            let resp = call_method(
                _interp,
                self_val.0,
                "voidcmd",
                &[Value::string(format!("MKD {}", dirname.0))],
            )?;
            let rs = value_to_str(_interp, &resp)?;
            if !rs.starts_with("257") {
                return Ok(Value::string(""));
            }
            Ok(Value::string(parse257_impl(_interp, &rs)?))
        }

        fn rmd(
            #[positional_only] self_val: PyValueRef<'_>,
            dirname: PyStr<'_>,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "voidcmd",
                &[Value::string(format!("RMD {}", dirname.0))],
            )
        }

        fn pwd(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let resp = call_method(_interp, self_val.0, "voidcmd", &[Value::string("PWD")])?;
            let rs = value_to_str(_interp, &resp)?;
            if !rs.starts_with("257") {
                return Ok(Value::string(""));
            }
            Ok(Value::string(parse257_impl(_interp, &rs)?))
        }

        fn quit(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let resp = call_method(_interp, self_val.0, "voidcmd", &[Value::string("QUIT")])?;
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
                call_method(_interp, &sock, "close", &[])
            } else {
                Ok(Value::none())
            };
            file_res?;
            sock_res?;
            Ok(Value::none())
        }
    }

    // ── FTP_TLS ──────────────────────────────────────────────────────────────

    class FTP_TLS("FTP") {
        #[allow(clippy::too_many_arguments)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] host: PyStr<'_>,
            #[default("".into())] user: PyStr<'_>,
            #[default("".into())] passwd: PyStr<'_>,
            #[default("".into())] acct: PyStr<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            context: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            timeout: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            source_address: OptValueRef<'_>,
            #[keyword_only]
            #[default("utf-8".into())]
            encoding: PyStr<'_>,
        ) -> Result<Value> {
            let ctx = match context.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let ssl_mod = _interp.load_module("ssl")?;
                    let mk_ctx = _interp.get_attr(&ssl_mod, "_create_stdlib_context")?;
                    call_pos(_interp, mk_ctx, &[])?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "context", ctx)?;
            _interp.assign_attr(self_val.0.clone(), "_prot_p", Value::bool_(false))?;
            let timeout_val = match timeout.as_value() {
                Some(v) => v.clone(),
                None => global_default_timeout(_interp)?,
            };
            let src_val = source_address
                .as_value()
                .cloned()
                .unwrap_or_else(Value::none);
            ftp_init_impl(
                _interp,
                self_val.0,
                &host.0,
                &user.0,
                &passwd.0,
                &acct.0,
                timeout_val,
                src_val,
                &encoding.0,
            )?;
            Ok(Value::none())
        }

        fn login(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] user: PyStr<'_>,
            #[default("".into())] passwd: PyStr<'_>,
            #[default("".into())] acct: PyStr<'_>,
            #[default(true.into())] secure: PyBool,
        ) -> Result<Value> {
            let sock = _interp.get_attr(self_val.0, "sock")?;
            if secure.0 && !is_ssl_socket(_interp, &sock)? {
                call_method(_interp, self_val.0, "auth", &[])?;
            }
            ftp_login_impl(_interp, self_val.0, &user.0, &passwd.0, &acct.0)
        }

        fn auth(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let sock = _interp.get_attr(self_val.0, "sock")?;
            if is_ssl_socket(_interp, &sock)? {
                return Err(PyError::named("ValueError", "Already using TLS"));
            }
            let ctx = _interp.get_attr(self_val.0, "context")?;
            let ssl_mod = _interp.load_module("ssl")?;
            let proto_tls = _interp.get_attr(&ssl_mod, "PROTOCOL_TLS")?;
            let ctx_proto = _interp.get_attr(&ctx, "protocol")?;
            let use_tls = _interp
                .eval_binary(ctx_proto, crate::ast::BinaryOp::Ge, proto_tls)?
                .as_bool();
            let cmd = if use_tls { "AUTH TLS" } else { "AUTH SSL" };
            let resp = call_method(_interp, self_val.0, "voidcmd", &[Value::string(cmd)])?;
            let host = _interp.get_attr(self_val.0, "host")?;
            let wrapped = call_method_kw(
                _interp,
                &ctx,
                "wrap_socket",
                &[sock],
                &[("server_hostname", host)],
            )?;
            let encoding = get_str_attr(_interp, self_val.0, "encoding", "utf-8")?;
            let file = call_method_kw(
                _interp,
                &wrapped,
                "makefile",
                &[Value::string("r")],
                &[("encoding", Value::string(encoding))],
            )?;
            _interp.assign_attr(self_val.0.clone(), "sock", wrapped)?;
            _interp.assign_attr(self_val.0.clone(), "file", file)?;
            Ok(resp)
        }

        fn ccc(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let sock = _interp.get_attr(self_val.0, "sock")?;
            if !is_ssl_socket(_interp, &sock)? {
                return Err(PyError::named("ValueError", "not using TLS"));
            }
            let resp = call_method(_interp, self_val.0, "voidcmd", &[Value::string("CCC")])?;
            let unwrapped = call_method(_interp, &sock, "unwrap", &[])?;
            let encoding = get_str_attr(_interp, self_val.0, "encoding", "utf-8")?;
            let file = call_method_kw(
                _interp,
                &unwrapped,
                "makefile",
                &[Value::string("r")],
                &[("encoding", Value::string(encoding))],
            )?;
            _interp.assign_attr(self_val.0.clone(), "sock", unwrapped)?;
            _interp.assign_attr(self_val.0.clone(), "file", file)?;
            Ok(resp)
        }

        fn prot_p(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "voidcmd", &[Value::string("PBSZ 0")])?;
            let resp = call_method(_interp, self_val.0, "voidcmd", &[Value::string("PROT P")])?;
            _interp.assign_attr(self_val.0.clone(), "_prot_p", Value::bool_(true))?;
            Ok(resp)
        }

        fn prot_c(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let resp = call_method(_interp, self_val.0, "voidcmd", &[Value::string("PROT C")])?;
            _interp.assign_attr(self_val.0.clone(), "_prot_p", Value::bool_(false))?;
            Ok(resp)
        }

        fn ntransfercmd(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
            #[default(OptionalArg::Omitted)] rest: OptValueRef<'_>,
        ) -> Result<Value> {
            let rest_opt = rest.as_value().filter(|v| !v.is_none()).cloned();
            let (mut conn, size) = ftp_ntransfercmd_impl(_interp, self_val.0, &cmd.0, rest_opt)?;
            let prot_p = {
                let v = _interp.get_attr(self_val.0, "_prot_p")?;
                _interp.truthy_value(&v)?
            };
            if prot_p {
                let ctx = _interp.get_attr(self_val.0, "context")?;
                let host = _interp.get_attr(self_val.0, "host")?;
                conn = call_method_kw(
                    _interp,
                    &ctx,
                    "wrap_socket",
                    &[conn],
                    &[("server_hostname", host)],
                )?;
            }
            Ok(Value::tuple(vec![conn, size]))
        }

        fn abort(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let line = Value::bytes(b"ABOR\r\n".to_vec());
            let sock = _interp.get_attr(self_val.0, "sock")?;
            call_method(_interp, &sock, "sendall", &[line])?;
            let resp_val = call_method(_interp, self_val.0, "getmultiline", &[])?;
            let resp = value_to_str(_interp, &resp_val)?;
            let code: String = resp.chars().take(3).collect();
            if !matches!(code.as_str(), "426" | "225" | "226") {
                return Err(raise_ftplib_err(_interp, "error_proto", &resp));
            }
            Ok(Value::string(resp))
        }
    }

    // ── Module-level helper functions ────────────────────────────────────────

    fn parse150(resp: PyStr<'_>) -> Result<Value> {
        parse150_impl(_interp, &resp.0)
    }

    fn parse227(resp: PyStr<'_>) -> Result<Value> {
        let (host, port) = parse227_impl(_interp, &resp.0)?;
        Ok(Value::tuple(vec![Value::string(host), Value::int(port)]))
    }

    fn parse229(resp: PyStr<'_>, peer: PyValue) -> Result<Value> {
        let (host, port) = parse229_impl(_interp, &resp.0, &peer.0)?;
        Ok(Value::tuple(vec![host, Value::int(port)]))
    }

    fn parse257(resp: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(parse257_impl(_interp, &resp.0)?))
    }

    fn print_line(line: PyValue) -> Result<Value> {
        call_pos(_interp, Value::builtin_function("print"), &[line.0])
    }

    fn ftpcp(
        source: PyValue,
        sourcename: PyStr<'_>,
        target: PyValue,
        #[default("".into())] targetname: PyStr<'_>,
        #[default("I".into())] r#type: PyStr<'_>,
    ) -> Result<Value> {
        let tgt_name = if targetname.0.is_empty() {
            sourcename.0.as_ref()
        } else {
            targetname.0.as_ref()
        };
        let type_cmd = Value::string(format!("TYPE {}", r#type.0));
        call_method(_interp, &source.0, "voidcmd", std::slice::from_ref(&type_cmd))?;
        call_method(_interp, &target.0, "voidcmd", &[type_cmd])?;
        let pasv_resp = call_method(_interp, &source.0, "sendcmd", &[Value::string("PASV")])?;
        let pasv_str = value_to_str(_interp, &pasv_resp)?;
        let (src_host, src_port) = parse227_impl(_interp, &pasv_str)?;
        call_method(
            _interp,
            &target.0,
            "sendport",
            &[Value::string(src_host), Value::int(src_port)],
        )?;
        let treply = call_method(
            _interp,
            &target.0,
            "sendcmd",
            &[Value::string(format!("STOR {tgt_name}"))],
        )?;
        let treply_str = value_to_str(_interp, &treply)?;
        if !treply_str.starts_with("125") && !treply_str.starts_with("150") {
            return Err(raise_ftplib_err(_interp, "error_proto", ""));
        }
        let sreply = call_method(
            _interp,
            &source.0,
            "sendcmd",
            &[Value::string(format!("RETR {}", sourcename.0))],
        )?;
        let sreply_str = value_to_str(_interp, &sreply)?;
        if !sreply_str.starts_with("125") && !sreply_str.starts_with("150") {
            return Err(raise_ftplib_err(_interp, "error_proto", ""));
        }
        call_method(_interp, &source.0, "voidresp", &[])?;
        call_method(_interp, &target.0, "voidresp", &[])?;
        Ok(Value::none())
    }
}
