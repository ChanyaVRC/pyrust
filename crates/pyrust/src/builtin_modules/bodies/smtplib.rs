// `smtplib` module — 100% native-Rust Python 3.12 SMTP, SMTP-over-SSL, and
// LMTP client implementation built over `socket` and `ssl` using PyRust's
// macro toolkit (issue #3258).
//
// Reference: <https://docs.python.org/3.12/library/smtplib.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, PyErrorKind, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, lookup_exc_class, render_value_repr, value_type_name_str,
};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

const SMTP_PORT_VAL: i64 = 25;
const SMTP_SSL_PORT_VAL: i64 = 465;
const LMTP_PORT_VAL: i64 = 2003;
const CRLF_STR: &str = "\r\n";
const B_CRLF_BYTES: &[u8] = b"\r\n";
const MAXLINE_VAL: i64 = 8192;
const MAXCHALLENGE_VAL: i64 = 5;

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

fn is_same_object(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    Ok(interp
        .eval_binary(a.clone(), crate::ast::BinaryOp::Is, b.clone())?
        .as_bool())
}

fn err_message(err: &PyError) -> String {
    match err.kind() {
        PyErrorKind::Named(_, msg) | PyErrorKind::Class(_, msg) | PyErrorKind::Runtime(msg) => {
            msg.clone()
        }
        PyErrorKind::OsError { strerror, .. } => strerror.clone(),
        _ => err.to_string(),
    }
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

fn smtplib_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("smtplib")?;
    interp.get_attr(&m, name)
}

fn raise_smtp_exc(interp: &mut Interpreter, exc_name: &str, args: &[Value]) -> PyError {
    if let Ok(cls) = smtplib_attr(interp, exc_name)
        && let Ok(inst) = call_pos(interp, cls, args)
    {
        return PyError::raised(inst);
    }
    PyError::named("OSError", exc_name)
}

fn global_default_timeout(interp: &mut Interpreter) -> Result<Value> {
    let sock_mod = interp.load_module("socket")?;
    interp.get_attr(&sock_mod, "_GLOBAL_DEFAULT_TIMEOUT")
}

fn is_instance(interp: &mut Interpreter, obj: &Value, cls: Value) -> Result<bool> {
    let res = call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), cls],
    )?;
    interp.truthy_value(&res)
}

fn is_os_error(interp: &mut Interpreter, err: &PyError) -> bool {
    let os_cls = Value::py_class(lookup_exc_class("OSError").expect("OSError"));
    match err.kind() {
        PyErrorKind::Raised(exc) => is_instance(interp, exc, os_cls).unwrap_or(false),
        PyErrorKind::OsError { .. } => true,
        PyErrorKind::Named(name, _) => matches!(
            name.as_ref(),
            "OSError"
                | "ConnectionError"
                | "BrokenPipeError"
                | "ConnectionResetError"
                | "ConnectionAbortedError"
                | "ConnectionRefusedError"
                | "TimeoutError"
                | "SSLError"
        ),
        _ => false,
    }
}

fn is_smtp_exc(interp: &mut Interpreter, err: &PyError, name: &str) -> bool {
    let Ok(cls) = smtplib_attr(interp, name) else {
        return false;
    };
    match err.kind() {
        PyErrorKind::Raised(exc) => is_instance(interp, exc, cls).unwrap_or(false),
        PyErrorKind::Named(n, _) => n.as_ref() == name,
        _ => false,
    }
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

fn get_bool_attr(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    let v = interp.get_attr(obj, name)?;
    interp.truthy_value(&v)
}

// ── RFC 4648 Base64 codec (no line wrapping) ─────────────────────────────────

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode_str(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(B64_TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64_TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn base64_decode_bytes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &b in input {
        if b.is_ascii_whitespace() || b == b'=' {
            continue;
        }
        let val: u32 = match b {
            b'A'..=b'Z' => (b - b'A') as u32,
            b'a'..=b'z' => (b - b'a' + 26) as u32,
            b'0'..=b'9' => (b - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    out
}

// ── Address & Data Formatting Helpers ────────────────────────────────────────

fn parseaddr_pair(interp: &mut Interpreter, addrstring: &str) -> Result<(String, String)> {
    let email_utils = interp.load_module("email.utils")?;
    let parseaddr = interp.get_attr(&email_utils, "parseaddr")?;
    let pair = call_pos(interp, parseaddr, &[Value::string(addrstring)])?;
    let disp = py_getitem(interp, &pair, Value::int(0))?
        .as_str()
        .unwrap_or_default()
        .to_string();
    let addr = py_getitem(interp, &pair, Value::int(1))?
        .as_str()
        .unwrap_or_default()
        .to_string();
    Ok((disp, addr))
}

fn quoteaddr_impl(interp: &mut Interpreter, addrstring: &str) -> Result<String> {
    let (disp, addr) = parseaddr_pair(interp, addrstring)?;
    if disp.is_empty() && addr.is_empty() {
        if addrstring.trim().starts_with('<') {
            return Ok(addrstring.to_string());
        }
        return Ok(format!("<{addrstring}>"));
    }
    Ok(format!("<{addr}>"))
}

fn addr_only_impl(interp: &mut Interpreter, addrstring: &str) -> Result<String> {
    let (disp, addr) = parseaddr_pair(interp, addrstring)?;
    if disp.is_empty() && addr.is_empty() {
        return Ok(addrstring.to_string());
    }
    Ok(addr)
}

fn fix_eols_str(data: &str) -> String {
    let mut out = String::with_capacity(data.len() + 16);
    let mut chars = data.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push_str(CRLF_STR);
        } else if c == '\n' {
            out.push_str(CRLF_STR);
        } else {
            out.push(c);
        }
    }
    out
}

fn quote_periods_str(data: &str) -> String {
    let mut out = String::with_capacity(data.len() + 8);
    let mut at_line_start = true;
    for c in data.chars() {
        if at_line_start && c == '.' {
            out.push_str("..");
        } else {
            out.push(c);
        }
        at_line_start = c == '\n';
    }
    out
}

fn quote_periods_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 8);
    let mut at_line_start = true;
    for &b in data {
        if at_line_start && b == b'.' {
            out.push(b'.');
            out.push(b'.');
        } else {
            out.push(b);
        }
        at_line_start = b == b'\n';
    }
    out
}

fn quotedata_impl(data: &str) -> String {
    quote_periods_str(&fix_eols_str(data))
}

fn encode_ascii_str(s: &str) -> Result<Vec<u8>> {
    if !s.is_ascii() {
        return Err(PyError::named(
            "UnicodeEncodeError",
            "'ascii' codec can't encode non-ASCII character",
        ));
    }
    Ok(s.as_bytes().to_vec())
}

// ── Shared SMTP Core Helpers (used by SMTP, SMTP_SSL, LMTP) ──────────────────

fn smtp_init_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    host: &str,
    port: i64,
    local_hostname: Option<String>,
    timeout: Value,
    source_address: Value,
) -> Result<()> {
    interp.assign_attr(self_val.clone(), "_host", Value::string(host))?;
    interp.assign_attr(self_val.clone(), "timeout", timeout)?;
    interp.assign_attr(
        self_val.clone(),
        "esmtp_features",
        Value::dict(PyDict::default()),
    )?;
    interp.assign_attr(self_val.clone(), "command_encoding", Value::string("ascii"))?;
    interp.assign_attr(self_val.clone(), "source_address", source_address)?;
    interp.assign_attr(self_val.clone(), "_auth_challenge_count", Value::int(0))?;

    if !host.is_empty() {
        let pair = call_method(
            interp,
            self_val,
            "connect",
            &[Value::string(host), Value::int(port)],
        )?;
        let code_val = py_getitem(interp, &pair, Value::int(0))?;
        let msg_val = py_getitem(interp, &pair, Value::int(1))?;
        if code_val.as_int() != Some(220) {
            let _ = call_method(interp, self_val, "close", &[]);
            return Err(raise_smtp_exc(
                interp,
                "SMTPConnectError",
                &[code_val, msg_val],
            ));
        }
    }

    let local_hn = if let Some(lh) = local_hostname {
        lh
    } else {
        let sock_mod = interp.load_module("socket")?;
        let getfqdn = interp.get_attr(&sock_mod, "getfqdn")?;
        let fqdn_val = call_pos(interp, getfqdn, &[])?;
        let fqdn = fqdn_val.as_str().unwrap_or_default().to_string();
        if fqdn.contains('.') {
            fqdn
        } else {
            let mut addr = "127.0.0.1".to_string();
            if let Ok(ghn) = interp.get_attr(&sock_mod, "gethostname")
                && let Ok(hn) = call_pos(interp, ghn, &[])
                && let Ok(ghbn) = interp.get_attr(&sock_mod, "gethostbyname")
                && let Ok(ip_val) = call_pos(interp, ghbn, &[hn])
                && let Some(ip) = ip_val.as_str()
            {
                addr = ip.to_string();
            }
            format!("[{addr}]")
        }
    };
    interp.assign_attr(self_val.clone(), "local_hostname", Value::string(local_hn))?;
    Ok(())
}

fn smtp_get_socket_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    host: &str,
    port: i64,
    timeout: Value,
) -> Result<Value> {
    if !timeout.is_none() && !interp.truthy_value(&timeout)? {
        return Err(PyError::named(
            "ValueError",
            "Non-blocking socket (timeout=0) is not supported",
        ));
    }
    let dbg = get_int_attr(interp, self_val, "debuglevel", 0)?;
    let src_addr = interp
        .get_attr(self_val, "source_address")
        .unwrap_or_else(|_| Value::none());
    let addr = Value::tuple(vec![Value::string(host), Value::int(port)]);
    if dbg > 0 {
        let _ = call_method(
            interp,
            self_val,
            "_print_debug",
            &[Value::string("connect: to"), addr.clone(), src_addr.clone()],
        );
    }
    let sock_mod = interp.load_module("socket")?;
    let create_conn = interp.get_attr(&sock_mod, "create_connection")?;
    call_pos(interp, create_conn, &[addr, timeout, src_addr])
}

fn smtp_connect_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    host_in: &str,
    port_in: i64,
    source_address: Option<Value>,
) -> Result<Value> {
    if let Some(sa) = source_address
        && interp.truthy_value(&sa)?
    {
        interp.assign_attr(self_val.clone(), "source_address", sa)?;
    }

    let mut host = host_in.to_string();
    let mut port = port_in;
    if port == 0 {
        let first_colon = host.find(':');
        let last_colon = host.rfind(':');
        if let (Some(i), Some(j)) = (first_colon, last_colon)
            && i == j
        {
            let port_part = &host[i + 1..];
            port = port_part
                .parse::<i64>()
                .map_err(|_| PyError::named("OSError", "nonnumeric port"))?;
            host = host[..i].to_string();
        }
    }
    if port == 0 {
        port = get_int_attr(interp, self_val, "default_port", SMTP_PORT_VAL)?;
    }

    let timeout = interp.get_attr(self_val, "timeout")?;
    let sock = call_method(
        interp,
        self_val,
        "_get_socket",
        &[Value::string(&host), Value::int(port), timeout],
    )?;
    interp.assign_attr(self_val.clone(), "sock", sock)?;
    interp.assign_attr(self_val.clone(), "file", Value::none())?;
    let pair = call_method(interp, self_val, "getreply", &[])?;
    let dbg = get_int_attr(interp, self_val, "debuglevel", 0)?;
    if dbg > 0 {
        let msg = py_getitem(interp, &pair, Value::int(1))?;
        let r = render_value_repr(interp, &msg)?;
        let _ = call_method(
            interp,
            self_val,
            "_print_debug",
            &[Value::string("connect:"), Value::string(r)],
        );
    }
    Ok(pair)
}

/// Parse one EHLO response feature line (after the greeting line), matching
/// CPython's `OLDSTYLE_AUTH` and `r'(?P<feature>[A-Za-z0-9][A-Za-z0-9\-]*) ?'`.
fn parse_ehlo_line(line: &str, features: &mut PyDict) {
    // Check old-style `auth=...` (case-insensitive) at start of line.
    if line.len() >= 5 && line[..5].eq_ignore_ascii_case("auth=") {
        let params = &line[5..];
        let key = PyKey::str_from("auth");
        let existing = features
            .get(&key)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        features.insert(key, Value::string(format!("{existing} {params}")));
        return;
    }

    let bytes = line.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphanumeric() {
        return;
    }
    let mut end = 1;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-') {
        end += 1;
    }
    let feature = line[..end].to_ascii_lowercase();
    let params = line[end..].trim();
    let key = PyKey::str_from(&feature);
    if feature == "auth" {
        let existing = features
            .get(&key)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        features.insert(key, Value::string(format!("{existing} {params}")));
    } else {
        features.insert(key, Value::string(params));
    }
}

// ── Post-load initialization ─────────────────────────────────────────────────

pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // Alias `SMTP.vrfy = SMTP.verify`.
    let smtp_opt = module.borrow().attrs.get("SMTP").cloned();
    if let Some(smtp_val) = smtp_opt
        && let ValueKind::PyClass(smtp_rc) = smtp_val.kind()
    {
        let verify_fn = smtp_rc.borrow().attrs.get("verify").cloned();
        if let Some(f) = verify_fn {
            smtp_rc.borrow_mut().attrs.insert("vrfy".to_string(), f);
        }
    }
    Ok(())
}

// ── Native module definition ─────────────────────────────────────────────────

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("SMTPException"),
            Value::string("SMTPNotSupportedError"),
            Value::string("SMTPServerDisconnected"),
            Value::string("SMTPResponseException"),
            Value::string("SMTPSenderRefused"),
            Value::string("SMTPRecipientsRefused"),
            Value::string("SMTPDataError"),
            Value::string("SMTPConnectError"),
            Value::string("SMTPHeloError"),
            Value::string("SMTPAuthenticationError"),
            Value::string("quoteaddr"),
            Value::string("quotedata"),
            Value::string("SMTP"),
            Value::string("SMTP_SSL"),
        ]),
        "SMTP_PORT" => Value::int(SMTP_PORT_VAL),
        "SMTP_SSL_PORT" => Value::int(SMTP_SSL_PORT_VAL),
        "LMTP_PORT" => Value::int(LMTP_PORT_VAL),
        "CRLF" => Value::string(CRLF_STR),
        "bCRLF" => Value::bytes(B_CRLF_BYTES.to_vec()),
        "_MAXLINE" => Value::int(MAXLINE_VAL),
        "_MAXCHALLENGE" => Value::int(MAXCHALLENGE_VAL),
    }

    // ── Exception hierarchy ──────────────────────────────────────────────────

    /// Base class for all exceptions raised by smtplib.
    exception SMTPException(OSError);
    /// Command or option is not supported by the SMTP server.
    exception SMTPNotSupportedError(SMTPException);
    /// Not connected to any SMTP server or server disconnected unexpectedly.
    exception SMTPServerDisconnected(SMTPException);

    class SMTPResponseException("SMTPException") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            code: PyValue,
            msg: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "smtp_code", code.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "smtp_error", msg.0.clone())?;
            _interp.assign_attr(
                self_val.0.clone(),
                "args",
                Value::tuple(vec![code.0, msg.0]),
            )?;
            Ok(Value::none())
        }
    }

    class SMTPSenderRefused("SMTPResponseException") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            code: PyValue,
            msg: PyValue,
            sender: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "smtp_code", code.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "smtp_error", msg.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "sender", sender.0.clone())?;
            _interp.assign_attr(
                self_val.0.clone(),
                "args",
                Value::tuple(vec![code.0, msg.0, sender.0]),
            )?;
            Ok(Value::none())
        }
    }

    class SMTPRecipientsRefused("SMTPException") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            recipients: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "recipients", recipients.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "args", Value::tuple(vec![recipients.0]))?;
            Ok(Value::none())
        }
    }

    class SMTPDataError("SMTPResponseException") {}
    class SMTPConnectError("SMTPResponseException") {}
    class SMTPHeloError("SMTPResponseException") {}
    class SMTPAuthenticationError("SMTPResponseException") {}

    // ── Module-level helper functions ────────────────────────────────────────

    fn quoteaddr(addrstring: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(quoteaddr_impl(_interp, &addrstring.0)?))
    }

    fn quotedata(data: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::string(quotedata_impl(&data.0)))
    }

    // ── SMTP ─────────────────────────────────────────────────────────────────

    class SMTP {
        constants {
            "debuglevel" => Value::int(0),
            "sock" => Value::none(),
            "file" => Value::none(),
            "helo_resp" => Value::none(),
            "ehlo_msg" => Value::string("ehlo"),
            "ehlo_resp" => Value::none(),
            "does_esmtp" => Value::bool_(false),
            "default_port" => Value::int(SMTP_PORT_VAL),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] host: PyStr<'_>,
            #[default(0_i64.into())] port: PyIndex,
            #[default(OptionalArg::Omitted)] local_hostname: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] source_address: OptValueRef<'_>,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let lh = match local_hostname.as_value() {
                Some(v) if !v.is_none() => Some(value_to_str(_interp, v)?),
                _ => None,
            };
            let timeout_val = match timeout.as_value() {
                Some(v) => v.clone(),
                None => global_default_timeout(_interp)?,
            };
            let src_val = source_address
                .as_value()
                .cloned()
                .unwrap_or_else(Value::none);
            smtp_init_impl(
                _interp,
                self_val.0,
                &host.0,
                port_i64,
                lh,
                timeout_val,
                src_val,
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
            let res: Result<()> = (|| {
                match call_method(_interp, self_val.0, "docmd", &[Value::string("QUIT")]) {
                    Ok(pair) => {
                        let code = py_getitem(_interp, &pair, Value::int(0))?;
                        let msg = py_getitem(_interp, &pair, Value::int(1))?;
                        if code.as_int() != Some(221) {
                            return Err(raise_smtp_exc(
                                _interp,
                                "SMTPResponseException",
                                &[code, msg],
                            ));
                        }
                        Ok(())
                    }
                    Err(err) if is_smtp_exc(_interp, &err, "SMTPServerDisconnected") => Ok(()),
                    Err(err) => Err(err),
                }
            })();
            let _ = call_method(_interp, self_val.0, "close", &[]);
            res?;
            Ok(Value::none())
        }

        fn set_debuglevel(
            #[positional_only] self_val: PyValueRef<'_>,
            debuglevel: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "debuglevel", debuglevel.0)?;
            Ok(Value::none())
        }

        fn _print_debug(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let dbg = get_int_attr(_interp, self_val.0, "debuglevel", 0)?;
            let sys_mod = _interp.load_module("sys")?;
            let stderr = _interp.get_attr(&sys_mod, "stderr")?;
            let mut pos = Vec::with_capacity(args.len() + 1);
            if dbg > 1
                && let Ok(dt_mod) = _interp.load_module("datetime")
                && let Ok(dt_cls) = _interp.get_attr(&dt_mod, "datetime")
                && let Ok(now) = call_method(_interp, &dt_cls, "now", &[])
                && let Ok(t) = call_method(_interp, &now, "time", &[])
            {
                pos.push(t);
            }
            pos.extend_from_slice(args);
            call_kw(
                _interp,
                Value::builtin_function("print"),
                &pos,
                &[("file", stderr)],
            )?;
            Ok(Value::none())
        }

        fn _get_socket(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyStr<'_>,
            port: PyIndex,
            timeout: PyValue,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            smtp_get_socket_impl(_interp, self_val.0, &host.0, port_i64, timeout.0)
        }

        fn connect(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("localhost".into())] host: PyStr<'_>,
            #[default(0_i64.into())] port: PyIndex,
            #[default(OptionalArg::Omitted)] source_address: OptValueRef<'_>,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let src = source_address.as_value().cloned();
            smtp_connect_impl(_interp, self_val.0, &host.0, port_i64, src)
        }

        fn send(#[positional_only] self_val: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            let dbg = get_int_attr(_interp, self_val.0, "debuglevel", 0)?;
            if dbg > 0 {
                let r = render_value_repr(_interp, &s.0)?;
                let _ = call_method(
                    _interp,
                    self_val.0,
                    "_print_debug",
                    &[Value::string("send:"), Value::string(r)],
                );
            }
            let sock = _interp.get_attr(self_val.0, "sock")?;
            if !_interp.truthy_value(&sock)? {
                return Err(raise_smtp_exc(
                    _interp,
                    "SMTPServerDisconnected",
                    &[Value::string("please run connect() first")],
                ));
            }
            let payload = if s.0.as_str().is_some() {
                let enc = get_str_attr(_interp, self_val.0, "command_encoding", "ascii")?;
                call_method(_interp, &s.0, "encode", &[Value::string(enc)])?
            } else {
                s.0
            };
            match call_method(_interp, &sock, "sendall", &[payload]) {
                Ok(_) => Ok(Value::none()),
                Err(err) if is_os_error(_interp, &err) => {
                    let _ = call_method(_interp, self_val.0, "close", &[]);
                    Err(raise_smtp_exc(
                        _interp,
                        "SMTPServerDisconnected",
                        &[Value::string("Server not connected")],
                    ))
                }
                Err(err) => Err(err),
            }
        }

        fn putcmd(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
            #[default("".into())] args: PyStr<'_>,
        ) -> Result<Value> {
            let s = if args.0.is_empty() {
                cmd.0.to_string()
            } else {
                format!("{} {}", cmd.0, args.0)
            };
            if s.contains('\r') || s.contains('\n') {
                let escaped = s.replace('\n', "\\n").replace('\r', "\\r");
                return Err(PyError::named(
                    "ValueError",
                    format!(
                        "command and arguments contain prohibited newline characters: {escaped}"
                    ),
                ));
            }
            call_method(
                _interp,
                self_val.0,
                "send",
                &[Value::string(format!("{s}{CRLF_STR}"))],
            )?;
            Ok(Value::none())
        }

        fn getreply(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut file = _interp.get_attr(self_val.0, "file")?;
            if file.is_none() {
                let sock = _interp.get_attr(self_val.0, "sock")?;
                if sock.is_none() {
                    return Err(raise_smtp_exc(
                        _interp,
                        "SMTPServerDisconnected",
                        &[Value::string("please run connect() first")],
                    ));
                }
                file = call_method(_interp, &sock, "makefile", &[Value::string("rb")])?;
                _interp.assign_attr(self_val.0.clone(), "file", file.clone())?;
            }
            let dbg = get_int_attr(_interp, self_val.0, "debuglevel", 0)?;
            let mut resp_lines: Vec<Vec<u8>> = Vec::new();
            let errcode: i64;
            loop {
                let line_val = match call_method(
                    _interp,
                    &file,
                    "readline",
                    &[Value::int(MAXLINE_VAL + 1)],
                ) {
                    Ok(v) => v,
                    Err(err) if is_os_error(_interp, &err) => {
                        let _ = call_method(_interp, self_val.0, "close", &[]);
                        return Err(raise_smtp_exc(
                            _interp,
                            "SMTPServerDisconnected",
                            &[Value::string(format!(
                                "Connection unexpectedly closed: {}",
                                err_message(&err)
                            ))],
                        ));
                    }
                    Err(err) => return Err(err),
                };
                let line = match line_val.kind() {
                    ValueKind::Bytes(b) => b.as_ref().clone(),
                    _ => Vec::new(),
                };
                if line.is_empty() {
                    let _ = call_method(_interp, self_val.0, "close", &[]);
                    return Err(raise_smtp_exc(
                        _interp,
                        "SMTPServerDisconnected",
                        &[Value::string("Connection unexpectedly closed")],
                    ));
                }
                if dbg > 0 {
                    let r = render_value_repr(_interp, &line_val)?;
                    let _ = call_method(
                        _interp,
                        self_val.0,
                        "_print_debug",
                        &[Value::string("reply:"), Value::string(r)],
                    );
                }
                if (line.len() as i64) > MAXLINE_VAL {
                    let _ = call_method(_interp, self_val.0, "close", &[]);
                    return Err(raise_smtp_exc(
                        _interp,
                        "SMTPResponseException",
                        &[Value::int(500), Value::string("Line too long.")],
                    ));
                }
                let body = if line.len() > 4 { &line[4..] } else { b"" };
                let start = body
                    .iter()
                    .position(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
                    .unwrap_or(body.len());
                let end = body
                    .iter()
                    .rposition(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
                    .map(|i| i + 1)
                    .unwrap_or(start);
                resp_lines.push(body[start..end].to_vec());

                let code_bytes = if line.len() >= 3 { &line[..3] } else { &line };
                match std::str::from_utf8(code_bytes)
                    .ok()
                    .and_then(|s| s.trim().parse::<i64>().ok())
                {
                    Some(c) => {
                        if line.get(3) != Some(&b'-') {
                            errcode = c;
                            break;
                        }
                    }
                    None => {
                        errcode = -1;
                        break;
                    }
                }
            }
            let errmsg = resp_lines.join(&b'\n');
            let errmsg_val = Value::bytes(errmsg);
            if dbg > 0 {
                let r = render_value_repr(_interp, &errmsg_val)?;
                let _ = call_method(
                    _interp,
                    self_val.0,
                    "_print_debug",
                    &[Value::string(format!("reply: retcode ({errcode}); Msg: {r}"))],
                );
            }
            Ok(Value::tuple(vec![Value::int(errcode), errmsg_val]))
        }

        fn docmd(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
            #[default("".into())] args: PyStr<'_>,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "putcmd",
                &[Value::string(&*cmd.0), Value::string(&*args.0)],
            )?;
            call_method(_interp, self_val.0, "getreply", &[])
        }

        fn helo(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] name: PyStr<'_>,
        ) -> Result<Value> {
            let arg = if name.0.is_empty() {
                get_str_attr(_interp, self_val.0, "local_hostname", "")?
            } else {
                name.0.to_string()
            };
            call_method(
                _interp,
                self_val.0,
                "putcmd",
                &[Value::string("helo"), Value::string(arg)],
            )?;
            let pair = call_method(_interp, self_val.0, "getreply", &[])?;
            let msg = py_getitem(_interp, &pair, Value::int(1))?;
            _interp.assign_attr(self_val.0.clone(), "helo_resp", msg)?;
            Ok(pair)
        }

        fn ehlo(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] name: PyStr<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "esmtp_features",
                Value::dict(PyDict::default()),
            )?;
            let ehlo_msg = get_str_attr(_interp, self_val.0, "ehlo_msg", "ehlo")?;
            let arg = if name.0.is_empty() {
                get_str_attr(_interp, self_val.0, "local_hostname", "")?
            } else {
                name.0.to_string()
            };
            call_method(
                _interp,
                self_val.0,
                "putcmd",
                &[Value::string(ehlo_msg), Value::string(arg)],
            )?;
            let pair = call_method(_interp, self_val.0, "getreply", &[])?;
            let code = py_getitem(_interp, &pair, Value::int(0))?
                .as_int()
                .unwrap_or(-1);
            let msg = py_getitem(_interp, &pair, Value::int(1))?;
            let msg_bytes = match msg.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => Vec::new(),
            };
            if code == -1 && msg_bytes.is_empty() {
                let _ = call_method(_interp, self_val.0, "close", &[]);
                return Err(raise_smtp_exc(
                    _interp,
                    "SMTPServerDisconnected",
                    &[Value::string("Server not connected")],
                ));
            }
            _interp.assign_attr(self_val.0.clone(), "ehlo_resp", msg)?;
            if code != 250 {
                return Ok(pair);
            }
            _interp.assign_attr(self_val.0.clone(), "does_esmtp", Value::bool_(true))?;

            let decoded: String = msg_bytes.iter().map(|&b| b as char).collect();
            let mut features = PyDict::default();
            for line in decoded.split('\n').skip(1) {
                parse_ehlo_line(line, &mut features);
            }
            _interp.assign_attr(self_val.0.clone(), "esmtp_features", Value::dict(features))?;
            Ok(pair)
        }

        fn has_extn(
            #[positional_only] self_val: PyValueRef<'_>,
            opt: PyStr<'_>,
        ) -> Result<Value> {
            let feats = _interp.get_attr(self_val.0, "esmtp_features")?;
            let low = opt.0.to_ascii_lowercase();
            let found = feats
                .as_dict()
                .is_some_and(|d| d.contains_key(&PyKey::str_from(&low)));
            Ok(Value::bool_(found))
        }

        fn help(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] args: PyStr<'_>,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "putcmd",
                &[Value::string("help"), Value::string(&*args.0)],
            )?;
            let pair = call_method(_interp, self_val.0, "getreply", &[])?;
            py_getitem(_interp, &pair, Value::int(1))
        }

        fn rset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "command_encoding", Value::string("ascii"))?;
            call_method(_interp, self_val.0, "docmd", &[Value::string("rset")])
        }

        fn _rset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            match call_method(_interp, self_val.0, "rset", &[]) {
                Ok(_) => Ok(Value::none()),
                Err(err) if is_smtp_exc(_interp, &err, "SMTPServerDisconnected") => {
                    Ok(Value::none())
                }
                Err(err) => Err(err),
            }
        }

        fn noop(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "docmd", &[Value::string("noop")])
        }

        fn mail(
            #[positional_only] self_val: PyValueRef<'_>,
            sender: PyStr<'_>,
            #[default(OptionalArg::Omitted)] options: OptValueRef<'_>,
        ) -> Result<Value> {
            let mut optionlist = String::new();
            let does_esmtp = get_bool_attr(_interp, self_val.0, "does_esmtp")?;
            if let Some(opts_val) = options.as_value() {
                let opts = _interp.collect_iterable(opts_val)?;
                if !opts.is_empty() && does_esmtp {
                    let mut opt_strs = Vec::with_capacity(opts.len());
                    for o in &opts {
                        let s = value_to_str(_interp, o)?;
                        if s.eq_ignore_ascii_case("smtputf8") {
                            let has = call_method(
                                _interp,
                                self_val.0,
                                "has_extn",
                                &[Value::string("smtputf8")],
                            )?;
                            if _interp.truthy_value(&has)? {
                                _interp.assign_attr(
                                    self_val.0.clone(),
                                    "command_encoding",
                                    Value::string("utf-8"),
                                )?;
                            } else {
                                return Err(raise_smtp_exc(
                                    _interp,
                                    "SMTPNotSupportedError",
                                    &[Value::string("SMTPUTF8 not supported by server")],
                                ));
                            }
                        }
                        opt_strs.push(s);
                    }
                    optionlist = format!(" {}", opt_strs.join(" "));
                }
            }
            let quoted = quoteaddr_impl(_interp, &sender.0)?;
            call_method(
                _interp,
                self_val.0,
                "putcmd",
                &[
                    Value::string("mail"),
                    Value::string(format!("FROM:{quoted}{optionlist}")),
                ],
            )?;
            call_method(_interp, self_val.0, "getreply", &[])
        }

        fn rcpt(
            #[positional_only] self_val: PyValueRef<'_>,
            recip: PyStr<'_>,
            #[default(OptionalArg::Omitted)] options: OptValueRef<'_>,
        ) -> Result<Value> {
            let mut optionlist = String::new();
            let does_esmtp = get_bool_attr(_interp, self_val.0, "does_esmtp")?;
            if let Some(opts_val) = options.as_value() {
                let opts = _interp.collect_iterable(opts_val)?;
                if !opts.is_empty() && does_esmtp {
                    let mut opt_strs = Vec::with_capacity(opts.len());
                    for o in &opts {
                        opt_strs.push(value_to_str(_interp, o)?);
                    }
                    optionlist = format!(" {}", opt_strs.join(" "));
                }
            }
            let quoted = quoteaddr_impl(_interp, &recip.0)?;
            call_method(
                _interp,
                self_val.0,
                "putcmd",
                &[
                    Value::string("rcpt"),
                    Value::string(format!("TO:{quoted}{optionlist}")),
                ],
            )?;
            call_method(_interp, self_val.0, "getreply", &[])
        }

        fn data(#[positional_only] self_val: PyValueRef<'_>, msg: PyValue) -> Result<Value> {
            call_method(_interp, self_val.0, "putcmd", &[Value::string("data")])?;
            let pair = call_method(_interp, self_val.0, "getreply", &[])?;
            let code = py_getitem(_interp, &pair, Value::int(0))?;
            let repl = py_getitem(_interp, &pair, Value::int(1))?;
            let dbg = get_int_attr(_interp, self_val.0, "debuglevel", 0)?;
            if dbg > 0 {
                let _ = call_method(
                    _interp,
                    self_val.0,
                    "_print_debug",
                    &[Value::string("data:"), pair],
                );
            }
            if code.as_int() != Some(354) {
                return Err(raise_smtp_exc(_interp, "SMTPDataError", &[code, repl]));
            }
            let raw_bytes: Vec<u8> = if let Some(s) = msg.0.as_str() {
                encode_ascii_str(&fix_eols_str(s))?
            } else if let ValueKind::Bytes(b) = msg.0.kind() {
                b.as_ref().clone()
            } else if let Some(ba) = pyrust_builtins::bytearray::as_bytearray_snapshot(&msg.0) {
                ba
            } else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "expected str or bytes-like object, got {}",
                        value_type_name_str(&msg.0)
                    ),
                ));
            };
            let mut q = quote_periods_bytes(&raw_bytes);
            if !q.ends_with(B_CRLF_BYTES) {
                q.extend_from_slice(B_CRLF_BYTES);
            }
            q.extend_from_slice(b".\r\n");
            call_method(_interp, self_val.0, "send", &[Value::bytes(q)])?;
            let final_pair = call_method(_interp, self_val.0, "getreply", &[])?;
            if dbg > 0 {
                let _ = call_method(
                    _interp,
                    self_val.0,
                    "_print_debug",
                    &[Value::string("data:"), final_pair.clone()],
                );
            }
            Ok(final_pair)
        }

        fn verify(
            #[positional_only] self_val: PyValueRef<'_>,
            address: PyStr<'_>,
        ) -> Result<Value> {
            let addr = addr_only_impl(_interp, &address.0)?;
            call_method(
                _interp,
                self_val.0,
                "putcmd",
                &[Value::string("vrfy"), Value::string(addr)],
            )?;
            call_method(_interp, self_val.0, "getreply", &[])
        }

        fn expn(
            #[positional_only] self_val: PyValueRef<'_>,
            address: PyStr<'_>,
        ) -> Result<Value> {
            let addr = addr_only_impl(_interp, &address.0)?;
            call_method(
                _interp,
                self_val.0,
                "putcmd",
                &[Value::string("expn"), Value::string(addr)],
            )?;
            call_method(_interp, self_val.0, "getreply", &[])
        }

        fn ehlo_or_helo_if_needed(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let helo_resp = _interp.get_attr(self_val.0, "helo_resp")?;
            let ehlo_resp = _interp.get_attr(self_val.0, "ehlo_resp")?;
            if helo_resp.is_none() && ehlo_resp.is_none() {
                let ehlo_pair = call_method(_interp, self_val.0, "ehlo", &[])?;
                let ecode = py_getitem(_interp, &ehlo_pair, Value::int(0))?
                    .as_int()
                    .unwrap_or(-1);
                if !(200..=299).contains(&ecode) {
                    let helo_pair = call_method(_interp, self_val.0, "helo", &[])?;
                    let hcode_val = py_getitem(_interp, &helo_pair, Value::int(0))?;
                    let hresp_val = py_getitem(_interp, &helo_pair, Value::int(1))?;
                    let hcode = hcode_val.as_int().unwrap_or(-1);
                    if !(200..=299).contains(&hcode) {
                        return Err(raise_smtp_exc(
                            _interp,
                            "SMTPHeloError",
                            &[hcode_val, hresp_val],
                        ));
                    }
                }
            }
            Ok(Value::none())
        }

        fn auth(
            #[positional_only] self_val: PyValueRef<'_>,
            mechanism: PyStr<'_>,
            authobject: PyValue,
            #[keyword_only]
            #[default(true.into())]
            initial_response_ok: PyBool,
        ) -> Result<Value> {
            let mech = mechanism.0.to_ascii_uppercase();
            let initial_response = if initial_response_ok.0 {
                call_pos(_interp, authobject.0.clone(), &[])?
            } else {
                Value::none()
            };
            let (mut code_val, mut resp_val) = if !initial_response.is_none() {
                let ir_str = value_to_str(_interp, &initial_response)?;
                let ir_bytes = encode_ascii_str(&ir_str)?;
                let b64 = base64_encode_str(&ir_bytes);
                let pair = call_method(
                    _interp,
                    self_val.0,
                    "docmd",
                    &[Value::string("AUTH"), Value::string(format!("{mech} {b64}"))],
                )?;
                _interp.assign_attr(self_val.0.clone(), "_auth_challenge_count", Value::int(1))?;
                (
                    py_getitem(_interp, &pair, Value::int(0))?,
                    py_getitem(_interp, &pair, Value::int(1))?,
                )
            } else {
                let pair = call_method(
                    _interp,
                    self_val.0,
                    "docmd",
                    &[Value::string("AUTH"), Value::string(&mech)],
                )?;
                _interp.assign_attr(self_val.0.clone(), "_auth_challenge_count", Value::int(0))?;
                (
                    py_getitem(_interp, &pair, Value::int(0))?,
                    py_getitem(_interp, &pair, Value::int(1))?,
                )
            };

            while code_val.as_int() == Some(334) {
                let cnt = get_int_attr(_interp, self_val.0, "_auth_challenge_count", 0)? + 1;
                _interp.assign_attr(self_val.0.clone(), "_auth_challenge_count", Value::int(cnt))?;
                let raw_resp = match resp_val.kind() {
                    ValueKind::Bytes(b) => b.as_ref().clone(),
                    _ => Vec::new(),
                };
                let challenge = base64_decode_bytes(&raw_resp);
                let ans = call_pos(_interp, authobject.0.clone(), &[Value::bytes(challenge)])?;
                let ans_str = value_to_str(_interp, &ans)?;
                let ans_bytes = encode_ascii_str(&ans_str)?;
                let b64 = base64_encode_str(&ans_bytes);
                let pair = call_method(_interp, self_val.0, "docmd", &[Value::string(b64)])?;
                code_val = py_getitem(_interp, &pair, Value::int(0))?;
                resp_val = py_getitem(_interp, &pair, Value::int(1))?;
                if cnt > MAXCHALLENGE_VAL {
                    let r = render_value_repr(_interp, &pair)?;
                    return Err(raise_smtp_exc(
                        _interp,
                        "SMTPException",
                        &[Value::string(format!(
                            "Server AUTH mechanism infinite loop. Last response: {r}"
                        ))],
                    ));
                }
            }
            if matches!(code_val.as_int(), Some(235 | 503)) {
                return Ok(Value::tuple(vec![code_val, resp_val]));
            }
            Err(raise_smtp_exc(
                _interp,
                "SMTPAuthenticationError",
                &[code_val, resp_val],
            ))
        }

        fn auth_cram_md5(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] challenge: OptValueRef<'_>,
        ) -> Result<Value> {
            let Some(ch) = challenge.as_value().filter(|v| !v.is_none()) else {
                return Ok(Value::none());
            };
            let user = get_str_attr(_interp, self_val.0, "user", "")?;
            let password = get_str_attr(_interp, self_val.0, "password", "")?;
            let pwd_bytes = encode_ascii_str(&password)?;
            let hmac_mod = _interp.load_module("hmac")?;
            let hmac_cls = _interp.get_attr(&hmac_mod, "HMAC")?;
            let h = call_pos(
                _interp,
                hmac_cls,
                &[Value::bytes(pwd_bytes), ch.clone(), Value::string("md5")],
            )?;
            let hex_val = call_method(_interp, &h, "hexdigest", &[])?;
            let hex = value_to_str(_interp, &hex_val)?;
            Ok(Value::string(format!("{user} {hex}")))
        }

        fn auth_plain(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _challenge: OptValueRef<'_>,
        ) -> Result<Value> {
            let user = get_str_attr(_interp, self_val.0, "user", "")?;
            let password = get_str_attr(_interp, self_val.0, "password", "")?;
            Ok(Value::string(format!("\0{user}\0{password}")))
        }

        fn auth_login(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] challenge: OptValueRef<'_>,
        ) -> Result<Value> {
            let cnt = get_int_attr(_interp, self_val.0, "_auth_challenge_count", 0)?;
            if challenge.as_value().is_none_or(|v| v.is_none()) || cnt < 2 {
                _interp.get_attr(self_val.0, "user")
            } else {
                _interp.get_attr(self_val.0, "password")
            }
        }

        fn login(
            #[positional_only] self_val: PyValueRef<'_>,
            user: PyValue,
            password: PyValue,
            #[keyword_only]
            #[default(true.into())]
            initial_response_ok: PyBool,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "ehlo_or_helo_if_needed", &[])?;
            let has_auth = call_method(_interp, self_val.0, "has_extn", &[Value::string("auth")])?;
            if !_interp.truthy_value(&has_auth)? {
                return Err(raise_smtp_exc(
                    _interp,
                    "SMTPNotSupportedError",
                    &[Value::string("SMTP AUTH extension not supported by server.")],
                ));
            }
            let feats = _interp.get_attr(self_val.0, "esmtp_features")?;
            let auth_str = feats
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from("auth")).cloned())
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let advertised: Vec<&str> = auth_str.split_whitespace().collect();
            let preferred = ["CRAM-MD5", "PLAIN", "LOGIN"];
            let authlist: Vec<&str> = preferred
                .iter()
                .copied()
                .filter(|m| advertised.contains(m))
                .collect();
            if authlist.is_empty() {
                return Err(raise_smtp_exc(
                    _interp,
                    "SMTPException",
                    &[Value::string("No suitable authentication method found.")],
                ));
            }

            _interp.assign_attr(self_val.0.clone(), "user", user.0)?;
            _interp.assign_attr(self_val.0.clone(), "password", password.0)?;

            let mut last_err: Option<PyError> = None;
            for method in authlist {
                let method_name = format!("auth_{}", method.to_ascii_lowercase().replace('-', "_"));
                let auth_fn = _interp.get_attr(self_val.0, &method_name)?;
                match call_method_kw(
                    _interp,
                    self_val.0,
                    "auth",
                    &[Value::string(method), auth_fn],
                    &[("initial_response_ok", Value::bool_(initial_response_ok.0))],
                ) {
                    Ok(pair) => {
                        let code = py_getitem(_interp, &pair, Value::int(0))?
                            .as_int();
                        if matches!(code, Some(235 | 503)) {
                            return Ok(pair);
                        }
                    }
                    Err(err) if is_smtp_exc(_interp, &err, "SMTPAuthenticationError") => {
                        last_err = Some(err);
                    }
                    Err(err) => return Err(err),
                }
            }
            Err(last_err.unwrap_or_else(|| {
                raise_smtp_exc(
                    _interp,
                    "SMTPAuthenticationError",
                    &[Value::int(535), Value::bytes(b"Authentication failed".to_vec())],
                )
            }))
        }

        fn starttls(
            #[positional_only] self_val: PyValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            context: OptValueRef<'_>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "ehlo_or_helo_if_needed", &[])?;
            let has_stls =
                call_method(_interp, self_val.0, "has_extn", &[Value::string("starttls")])?;
            if !_interp.truthy_value(&has_stls)? {
                return Err(raise_smtp_exc(
                    _interp,
                    "SMTPNotSupportedError",
                    &[Value::string(
                        "STARTTLS extension not supported by server.",
                    )],
                ));
            }
            let pair = call_method(_interp, self_val.0, "docmd", &[Value::string("STARTTLS")])?;
            let resp_val = py_getitem(_interp, &pair, Value::int(0))?;
            let reply_val = py_getitem(_interp, &pair, Value::int(1))?;
            if resp_val.as_int() == Some(220) {
                let ctx = match context.as_value() {
                    Some(v) if !v.is_none() => v.clone(),
                    _ => {
                        let ssl_mod = _interp.load_module("ssl")?;
                        let mk = _interp.get_attr(&ssl_mod, "_create_stdlib_context")?;
                        call_pos(_interp, mk, &[])?
                    }
                };
                let sock = _interp.get_attr(self_val.0, "sock")?;
                let host = _interp.get_attr(self_val.0, "_host")?;
                let wrapped = call_method_kw(
                    _interp,
                    &ctx,
                    "wrap_socket",
                    &[sock],
                    &[("server_hostname", host)],
                )?;
                _interp.assign_attr(self_val.0.clone(), "sock", wrapped)?;
                _interp.assign_attr(self_val.0.clone(), "file", Value::none())?;
                _interp.assign_attr(self_val.0.clone(), "helo_resp", Value::none())?;
                _interp.assign_attr(self_val.0.clone(), "ehlo_resp", Value::none())?;
                _interp.assign_attr(
                    self_val.0.clone(),
                    "esmtp_features",
                    Value::dict(PyDict::default()),
                )?;
                _interp.assign_attr(self_val.0.clone(), "does_esmtp", Value::bool_(false))?;
                Ok(pair)
            } else {
                Err(raise_smtp_exc(
                    _interp,
                    "SMTPResponseException",
                    &[resp_val, reply_val],
                ))
            }
        }

        fn sendmail(
            #[positional_only] self_val: PyValueRef<'_>,
            from_addr: PyStr<'_>,
            to_addrs: PyValue,
            msg: PyValue,
            #[default(OptionalArg::Omitted)] mail_options: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] rcpt_options: OptValueRef<'_>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "ehlo_or_helo_if_needed", &[])?;
            let msg_bytes_val = if let Some(s) = msg.0.as_str() {
                Value::bytes(encode_ascii_str(&fix_eols_str(s))?)
            } else {
                msg.0
            };
            let msg_len = match msg_bytes_val.kind() {
                ValueKind::Bytes(b) => b.len(),
                _ => {
                    let lv = call_pos(
                        _interp,
                        Value::builtin_function("len"),
                        std::slice::from_ref(&msg_bytes_val),
                    )?;
                    lv.as_int().unwrap_or(0) as usize
                }
            };

            let mut esmtp_opts: Vec<Value> = Vec::new();
            if get_bool_attr(_interp, self_val.0, "does_esmtp")? {
                let has_size =
                    call_method(_interp, self_val.0, "has_extn", &[Value::string("size")])?;
                if _interp.truthy_value(&has_size)? {
                    esmtp_opts.push(Value::string(format!("size={msg_len}")));
                }
                if let Some(mo) = mail_options.as_value() {
                    let items = _interp.collect_iterable(mo)?;
                    esmtp_opts.extend(items);
                }
            }

            let mail_pair = call_method(
                _interp,
                self_val.0,
                "mail",
                &[Value::string(&*from_addr.0), Value::list(esmtp_opts)],
            )?;
            let code = py_getitem(_interp, &mail_pair, Value::int(0))?;
            let resp = py_getitem(_interp, &mail_pair, Value::int(1))?;
            if code.as_int() != Some(250) {
                if code.as_int() == Some(421) {
                    let _ = call_method(_interp, self_val.0, "close", &[]);
                } else {
                    let _ = call_method(_interp, self_val.0, "_rset", &[]);
                }
                return Err(raise_smtp_exc(
                    _interp,
                    "SMTPSenderRefused",
                    &[code, resp, Value::string(&*from_addr.0)],
                ));
            }

            let recipients: Vec<Value> = if to_addrs.0.as_str().is_some() {
                vec![to_addrs.0]
            } else {
                _interp.collect_iterable(&to_addrs.0)?
            };
            let rcpt_opts_val = rcpt_options
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::tuple(Vec::new()));

            let mut senderrs = PyDict::default();
            for each in &recipients {
                let rcpt_pair = call_method(
                    _interp,
                    self_val.0,
                    "rcpt",
                    &[each.clone(), rcpt_opts_val.clone()],
                )?;
                let rcode = py_getitem(_interp, &rcpt_pair, Value::int(0))?;
                let rresp = py_getitem(_interp, &rcpt_pair, Value::int(1))?;
                let rcode_i = rcode.as_int().unwrap_or(-1);
                if rcode_i != 250 && rcode_i != 251 {
                    let k = value_to_str(_interp, each)?;
                    senderrs.insert(PyKey::str_from(&k), Value::tuple(vec![rcode, rresp]));
                }
                if rcode_i == 421 {
                    let _ = call_method(_interp, self_val.0, "close", &[]);
                    return Err(raise_smtp_exc(
                        _interp,
                        "SMTPRecipientsRefused",
                        &[Value::dict(senderrs)],
                    ));
                }
            }
            if senderrs.len() == recipients.len() {
                let _ = call_method(_interp, self_val.0, "_rset", &[]);
                return Err(raise_smtp_exc(
                    _interp,
                    "SMTPRecipientsRefused",
                    &[Value::dict(senderrs)],
                ));
            }

            let data_pair = call_method(_interp, self_val.0, "data", &[msg_bytes_val])?;
            let dcode = py_getitem(_interp, &data_pair, Value::int(0))?;
            let dresp = py_getitem(_interp, &data_pair, Value::int(1))?;
            if dcode.as_int() != Some(250) {
                if dcode.as_int() == Some(421) {
                    let _ = call_method(_interp, self_val.0, "close", &[]);
                } else {
                    let _ = call_method(_interp, self_val.0, "_rset", &[]);
                }
                return Err(raise_smtp_exc(_interp, "SMTPDataError", &[dcode, dresp]));
            }
            Ok(Value::dict(senderrs))
        }

        fn send_message(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
            #[default(OptionalArg::Omitted)] from_addr: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] to_addrs: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] mail_options: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] rcpt_options: OptValueRef<'_>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "ehlo_or_helo_if_needed", &[])?;
            let resent = call_method(_interp, &msg.0, "get_all", &[Value::string("Resent-Date")])?;
            let header_prefix = if resent.is_none() {
                ""
            } else {
                let items = _interp.collect_iterable(&resent)?;
                if items.len() == 1 {
                    "Resent-"
                } else {
                    return Err(PyError::named(
                        "ValueError",
                        "message has more than one 'Resent-' header block",
                    ));
                }
            };

            let email_utils = _interp.load_module("email.utils")?;
            let getaddresses = _interp.get_attr(&email_utils, "getaddresses")?;

            let from_str = match from_addr.as_value() {
                Some(v) if !v.is_none() => value_to_str(_interp, v)?,
                _ => {
                    let sender_key = format!("{header_prefix}Sender");
                    let from_key = format!("{header_prefix}From");
                    let contains_val = call_method(
                        _interp,
                        &msg.0,
                        "__contains__",
                        &[Value::string(&sender_key)],
                    )?;
                    let has_sender = _interp.truthy_value(&contains_val)?;
                    let hdr_key = if has_sender { sender_key } else { from_key };
                    let hdr_val = py_getitem(_interp, &msg.0, Value::string(hdr_key))?;
                    let parsed =
                        call_pos(_interp, getaddresses.clone(), &[Value::list(vec![hdr_val])])?;
                    let first = py_getitem(_interp, &parsed, Value::int(0))?;
                    let addr = py_getitem(_interp, &first, Value::int(1))?;
                    value_to_str(_interp, &addr)?
                }
            };

            let to_list: Vec<String> = match to_addrs.as_value() {
                Some(v) if !v.is_none() => {
                    if let Some(s) = v.as_str() {
                        vec![s.to_string()]
                    } else {
                        let items = _interp.collect_iterable(v)?;
                        let mut out = Vec::with_capacity(items.len());
                        for item in items {
                            out.push(value_to_str(_interp, &item)?);
                        }
                        out
                    }
                }
                _ => {
                    let mut addr_fields = Vec::new();
                    for field in ["To", "Bcc", "Cc"] {
                        let key = format!("{header_prefix}{field}");
                        let v = py_getitem(_interp, &msg.0, Value::string(key))?;
                        if !v.is_none() {
                            addr_fields.push(v);
                        }
                    }
                    let parsed = call_pos(_interp, getaddresses, &[Value::list(addr_fields)])?;
                    let pairs = _interp.collect_iterable(&parsed)?;
                    let mut out = Vec::with_capacity(pairs.len());
                    for p in pairs {
                        let a = py_getitem(_interp, &p, Value::int(1))?;
                        out.push(value_to_str(_interp, &a)?);
                    }
                    out
                }
            };

            // Copy message and strip Bcc / Resent-Bcc headers.
            let copy_mod = _interp.load_module("copy")?;
            let copy_fn = _interp.get_attr(&copy_mod, "copy")?;
            let msg_copy = call_pos(_interp, copy_fn, std::slice::from_ref(&msg.0))?;
            let _ = call_method(_interp, &msg_copy, "__delitem__", &[Value::string("Bcc")]);
            let _ = call_method(
                _interp,
                &msg_copy,
                "__delitem__",
                &[Value::string("Resent-Bcc")],
            );

            let international = !from_str.is_ascii() || to_list.iter().any(|a| !a.is_ascii());
            if international {
                let has_utf8 =
                    call_method(_interp, self_val.0, "has_extn", &[Value::string("smtputf8")])?;
                if !_interp.truthy_value(&has_utf8)? {
                    return Err(raise_smtp_exc(
                        _interp,
                        "SMTPNotSupportedError",
                        &[Value::string(
                            "One or more source or delivery addresses require internationalized email support, but the server does not advertise the required SMTPUTF8 capability",
                        )],
                    ));
                }
            }

            let mut mopts: Vec<Value> = match mail_options.as_value() {
                Some(v) => _interp.collect_iterable(v)?,
                None => Vec::new(),
            };
            let policy = _interp.get_attr(&msg.0, "policy")?;
            let serialize_policy = if international {
                mopts.push(Value::string("SMTPUTF8"));
                mopts.push(Value::string("BODY=8BITMIME"));
                call_method_kw(
                    _interp,
                    &policy,
                    "clone",
                    &[],
                    &[
                        ("utf8", Value::bool_(true)),
                        ("linesep", Value::string(CRLF_STR)),
                    ],
                )?
            } else {
                call_method_kw(
                    _interp,
                    &policy,
                    "clone",
                    &[],
                    &[("linesep", Value::string(CRLF_STR))],
                )?
            };
            let flatmsg = call_method_kw(
                _interp,
                &msg_copy,
                "as_bytes",
                &[],
                &[("policy", serialize_policy)],
            )?;

            let to_val = Value::list(to_list.into_iter().map(Value::string).collect());
            let rcpt_val = rcpt_options
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::tuple(Vec::new()));
            call_method(
                _interp,
                self_val.0,
                "sendmail",
                &[
                    Value::string(from_str),
                    to_val,
                    flatmsg,
                    Value::tuple(mopts),
                    rcpt_val,
                ],
            )
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let file = _interp.get_attr(self_val.0, "file")?;
            _interp.assign_attr(self_val.0.clone(), "file", Value::none())?;
            let file_res = if _interp.truthy_value(&file)? {
                call_method(_interp, &file, "close", &[])
            } else {
                Ok(Value::none())
            };
            let sock = _interp.get_attr(self_val.0, "sock")?;
            _interp.assign_attr(self_val.0.clone(), "sock", Value::none())?;
            let sock_res = if _interp.truthy_value(&sock)? {
                call_method(_interp, &sock, "close", &[])
            } else {
                Ok(Value::none())
            };
            file_res?;
            sock_res?;
            Ok(Value::none())
        }

        fn quit(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let res = call_method(_interp, self_val.0, "docmd", &[Value::string("quit")])?;
            _interp.assign_attr(self_val.0.clone(), "ehlo_resp", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "helo_resp", Value::none())?;
            _interp.assign_attr(
                self_val.0.clone(),
                "esmtp_features",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "does_esmtp", Value::bool_(false))?;
            call_method(_interp, self_val.0, "close", &[])?;
            Ok(res)
        }
    }

    // ── SMTP_SSL ─────────────────────────────────────────────────────────────

    class SMTP_SSL("SMTP") {
        constants {
            "default_port" => Value::int(SMTP_SSL_PORT_VAL),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] host: PyStr<'_>,
            #[default(0_i64.into())] port: PyIndex,
            #[default(OptionalArg::Omitted)] local_hostname: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            timeout: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            source_address: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            context: OptValueRef<'_>,
        ) -> Result<Value> {
            let ctx = match context.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let ssl_mod = _interp.load_module("ssl")?;
                    let mk = _interp.get_attr(&ssl_mod, "_create_stdlib_context")?;
                    call_pos(_interp, mk, &[])?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "context", ctx)?;
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let lh = match local_hostname.as_value() {
                Some(v) if !v.is_none() => Some(value_to_str(_interp, v)?),
                _ => None,
            };
            let timeout_val = match timeout.as_value() {
                Some(v) => v.clone(),
                None => global_default_timeout(_interp)?,
            };
            let src_val = source_address
                .as_value()
                .cloned()
                .unwrap_or_else(Value::none);
            smtp_init_impl(
                _interp,
                self_val.0,
                &host.0,
                port_i64,
                lh,
                timeout_val,
                src_val,
            )?;
            Ok(Value::none())
        }

        fn _get_socket(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyStr<'_>,
            port: PyIndex,
            timeout: PyValue,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let dbg = get_int_attr(_interp, self_val.0, "debuglevel", 0)?;
            if dbg > 0 {
                let _ = call_method(
                    _interp,
                    self_val.0,
                    "_print_debug",
                    &[
                        Value::string("connect:"),
                        Value::tuple(vec![Value::string(&*host.0), Value::int(port_i64)]),
                    ],
                );
            }
            let new_sock = smtp_get_socket_impl(_interp, self_val.0, &host.0, port_i64, timeout.0)?;
            let ctx = _interp.get_attr(self_val.0, "context")?;
            let server_host = _interp.get_attr(self_val.0, "_host")?;
            call_method_kw(
                _interp,
                &ctx,
                "wrap_socket",
                &[new_sock],
                &[("server_hostname", server_host)],
            )
        }
    }

    // ── LMTP ─────────────────────────────────────────────────────────────────

    class LMTP("SMTP") {
        constants {
            "ehlo_msg" => Value::string("lhlo"),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] host: PyStr<'_>,
            #[default(LMTP_PORT_VAL.into())] port: PyIndex,
            #[default(OptionalArg::Omitted)] local_hostname: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] source_address: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let lh = match local_hostname.as_value() {
                Some(v) if !v.is_none() => Some(value_to_str(_interp, v)?),
                _ => None,
            };
            let timeout_val = match timeout.as_value() {
                Some(v) => v.clone(),
                None => global_default_timeout(_interp)?,
            };
            let src_val = source_address
                .as_value()
                .cloned()
                .unwrap_or_else(Value::none);
            smtp_init_impl(
                _interp,
                self_val.0,
                &host.0,
                port_i64,
                lh,
                timeout_val,
                src_val,
            )?;
            Ok(Value::none())
        }

        fn connect(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("localhost".into())] host: PyStr<'_>,
            #[default(0_i64.into())] port: PyIndex,
            #[default(OptionalArg::Omitted)] source_address: OptValueRef<'_>,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let src = source_address.as_value().cloned();
            if !host.0.starts_with('/') {
                return smtp_connect_impl(_interp, self_val.0, &host.0, port_i64, src);
            }
            let cur_timeout = _interp.get_attr(self_val.0, "timeout")?;
            if !cur_timeout.is_none() && !_interp.truthy_value(&cur_timeout)? {
                return Err(PyError::named(
                    "ValueError",
                    "Non-blocking socket (timeout=0) is not supported",
                ));
            }
            let sock_mod = _interp.load_module("socket")?;
            let af_unix = _interp.get_attr(&sock_mod, "AF_UNIX")?;
            let sock_stream = _interp.get_attr(&sock_mod, "SOCK_STREAM")?;
            let sock_cls = _interp.get_attr(&sock_mod, "socket")?;
            let sock = call_pos(_interp, sock_cls, &[af_unix, sock_stream])?;
            let gdt = global_default_timeout(_interp)?;
            if !is_same_object(_interp, &cur_timeout, &gdt)? {
                call_method(_interp, &sock, "settimeout", &[cur_timeout])?;
            }
            _interp.assign_attr(self_val.0.clone(), "sock", sock.clone())?;
            _interp.assign_attr(self_val.0.clone(), "file", Value::none())?;
            if let Err(err) = call_method(_interp, &sock, "connect", &[Value::string(&*host.0)]) {
                let _ = call_method(_interp, &sock, "close", &[]);
                _interp.assign_attr(self_val.0.clone(), "sock", Value::none())?;
                return Err(err);
            }
            call_method(_interp, self_val.0, "getreply", &[])
        }
    }
}
