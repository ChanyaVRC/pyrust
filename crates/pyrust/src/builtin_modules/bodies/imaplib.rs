// `imaplib` module — 100% native-Rust Python 3.12 IMAP4, IMAP4-over-SSL, and
// IMAP4-over-stream client implementation built over `socket` and `ssl` using
// PyRust's macro toolkit (issue #3258).
//
// Reference: <https://docs.python.org/3.12/library/imaplib.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, PyErrorKind, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, lookup_exc_class, render_value_repr};
use crate::value::{PyClass, PyDict, PyKey, PyModule, Value, ValueKind};

const IMAP4_PORT_VAL: i64 = 143;
const IMAP4_SSL_PORT_VAL: i64 = 993;
const MAXLINE_VAL: i64 = 1_000_000;
const CRLF_BYTES: &[u8] = b"\r\n";

const MONTHS: [&str; 13] = [
    "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const COMMANDS_SPEC: &[(&str, &[&str])] = &[
    ("APPEND", &["AUTH", "SELECTED"]),
    ("AUTHENTICATE", &["NONAUTH"]),
    ("CAPABILITY", &["NONAUTH", "AUTH", "SELECTED", "LOGOUT"]),
    ("CHECK", &["SELECTED"]),
    ("CLOSE", &["SELECTED"]),
    ("COPY", &["SELECTED"]),
    ("CREATE", &["AUTH", "SELECTED"]),
    ("DELETE", &["AUTH", "SELECTED"]),
    ("DELETEACL", &["AUTH", "SELECTED"]),
    ("ENABLE", &["AUTH"]),
    ("EXAMINE", &["AUTH", "SELECTED"]),
    ("EXPUNGE", &["SELECTED"]),
    ("FETCH", &["SELECTED"]),
    ("GETACL", &["AUTH", "SELECTED"]),
    ("GETANNOTATION", &["AUTH", "SELECTED"]),
    ("GETQUOTA", &["AUTH", "SELECTED"]),
    ("GETQUOTAROOT", &["AUTH", "SELECTED"]),
    ("MYRIGHTS", &["AUTH", "SELECTED"]),
    ("LIST", &["AUTH", "SELECTED"]),
    ("LOGIN", &["NONAUTH"]),
    ("LOGOUT", &["NONAUTH", "AUTH", "SELECTED", "LOGOUT"]),
    ("LSUB", &["AUTH", "SELECTED"]),
    ("MOVE", &["SELECTED"]),
    ("NAMESPACE", &["AUTH", "SELECTED"]),
    ("NOOP", &["NONAUTH", "AUTH", "SELECTED", "LOGOUT"]),
    ("PARTIAL", &["SELECTED"]),
    ("PROXYAUTH", &["AUTH"]),
    ("RENAME", &["AUTH", "SELECTED"]),
    ("SEARCH", &["SELECTED"]),
    ("SELECT", &["AUTH", "SELECTED"]),
    ("SETACL", &["AUTH", "SELECTED"]),
    ("SETANNOTATION", &["AUTH", "SELECTED"]),
    ("SETQUOTA", &["AUTH", "SELECTED"]),
    ("SORT", &["SELECTED"]),
    ("STARTTLS", &["NONAUTH"]),
    ("STATUS", &["AUTH", "SELECTED"]),
    ("STORE", &["SELECTED"]),
    ("SUBSCRIBE", &["AUTH", "SELECTED"]),
    ("THREAD", &["SELECTED"]),
    ("UID", &["SELECTED"]),
    ("UNSUBSCRIBE", &["AUTH", "SELECTED"]),
    ("UNSELECT", &["SELECTED"]),
];

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

fn imap_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("imaplib")?;
    interp.get_attr(&m, name)
}

fn raise_self_exc(
    interp: &mut Interpreter,
    self_val: &Value,
    exc_attr: &str,
    arg: Value,
) -> PyError {
    if let Ok(cls) = interp.get_attr(self_val, exc_attr)
        && let Ok(inst) = call_pos(interp, cls, std::slice::from_ref(&arg))
    {
        return PyError::raised(inst);
    }
    PyError::named("Exception", format!("{exc_attr}: {arg:?}"))
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

fn get_bool_attr(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    let v = interp.get_attr(obj, name)?;
    interp.truthy_value(&v)
}

// ── Base64 codec for _Authenticator ──────────────────────────────────────────

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_TABLE[((triple >> 18) & 0x3F) as usize]);
        out.push(B64_TABLE[((triple >> 12) & 0x3F) as usize]);
        out.push(if chunk.len() > 1 {
            B64_TABLE[((triple >> 6) & 0x3F) as usize]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            B64_TABLE[(triple & 0x3F) as usize]
        } else {
            b'='
        });
    }
    out
}

fn b64_decode(input: &[u8]) -> Vec<u8> {
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

// ── Public Helper Functions ──────────────────────────────────────────────────

fn int2ap_impl(num: i64) -> Vec<u8> {
    const AP: &[u8; 16] = b"ABCDEFGHIJKLMNOP";
    let mut n = num.unsigned_abs();
    let mut rev = Vec::new();
    while n > 0 {
        rev.push(AP[(n % 16) as usize]);
        n /= 16;
    }
    rev.reverse();
    rev
}

fn parse_flags_impl(resp: &[u8]) -> Vec<Value> {
    // Match `.*FLAGS \((?P<flags>[^\)]*)\)`
    let needle = b"FLAGS (";
    let Some(pos) = resp.windows(needle.len()).rposition(|w| w == needle) else {
        return Vec::new();
    };
    let rest = &resp[pos + needle.len()..];
    let Some(close) = rest.iter().position(|&b| b == b')') else {
        return Vec::new();
    };
    let inside = &rest[..close];
    inside
        .split(|b| b.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| Value::bytes(s.to_vec()))
        .collect()
}

fn month_name_to_num(mon: &[u8]) -> Option<i64> {
    for (idx, name) in MONTHS.iter().enumerate().skip(1) {
        if name.as_bytes() == mon {
            return Some(idx as i64);
        }
    }
    None
}

fn internaldate2tuple_impl(interp: &mut Interpreter, resp: &[u8]) -> Result<Value> {
    // Match `.*INTERNALDATE "DD-Mmm-YYYY HH:MM:SS (+|-)HHMM"`
    let needle = b"INTERNALDATE \"";
    let Some(pos) = resp.windows(needle.len()).rposition(|w| w == needle) else {
        return Ok(Value::none());
    };
    let rest = &resp[pos + needle.len()..];
    // Expected length: "DD-Mmm-YYYY HH:MM:SS +HHMM\"" => 26 chars + '"' = 27 bytes
    if rest.len() < 27 || rest[26] != b'"' {
        return Ok(Value::none());
    }
    let s = &rest[..26];
    if s[2] != b'-'
        || s[6] != b'-'
        || s[11] != b' '
        || s[14] != b':'
        || s[17] != b':'
        || s[20] != b' '
        || !matches!(s[21], b'+' | b'-')
    {
        return Ok(Value::none());
    }
    let day_ok = matches!(s[0], b' ' | b'0'..=b'3') && s[1].is_ascii_digit();
    let Some(mon) = month_name_to_num(&s[3..6]) else {
        return Ok(Value::none());
    };
    let digits_ok = day_ok
        && s[7..11].iter().all(u8::is_ascii_digit)
        && s[12..14].iter().all(u8::is_ascii_digit)
        && s[15..17].iter().all(u8::is_ascii_digit)
        && s[18..20].iter().all(u8::is_ascii_digit)
        && s[22..26].iter().all(u8::is_ascii_digit);
    if !digits_ok {
        return Ok(Value::none());
    }
    let parse_u = |slice: &[u8]| -> i64 {
        std::str::from_utf8(slice)
            .unwrap_or("0")
            .trim()
            .parse::<i64>()
            .unwrap_or(0)
    };
    let day = parse_u(&s[0..2]);
    let year = parse_u(&s[7..11]);
    let hour = parse_u(&s[12..14]);
    let min = parse_u(&s[15..17]);
    let sec = parse_u(&s[18..20]);
    let zoneh = parse_u(&s[22..24]);
    let zonem = parse_u(&s[24..26]);
    let mut zone = (zoneh * 60 + zonem) * 60;
    if s[21] == b'-' {
        zone = -zone;
    }

    let tt = Value::tuple(vec![
        Value::int(year),
        Value::int(mon),
        Value::int(day),
        Value::int(hour),
        Value::int(min),
        Value::int(sec),
        Value::int(-1),
        Value::int(-1),
        Value::int(-1),
    ]);
    let cal_mod = interp.load_module("calendar")?;
    let timegm = interp.get_attr(&cal_mod, "timegm")?;
    let utc_val = call_pos(interp, timegm, &[tt])?;
    let utc = utc_val.as_int().unwrap_or(0) - zone;

    let time_mod = interp.load_module("time")?;
    let localtime = interp.get_attr(&time_mod, "localtime")?;
    call_pos(interp, localtime, &[Value::int(utc)])
}

fn time2internaldate_impl(interp: &mut Interpreter, date_time: &Value) -> Result<Value> {
    let dt_mod = interp.load_module("datetime")?;
    let dt_cls = interp.get_attr(&dt_mod, "datetime")?;
    let tz_cls = interp.get_attr(&dt_mod, "timezone")?;
    let td_cls = interp.get_attr(&dt_mod, "timedelta")?;

    let dt = if matches!(date_time.kind(), ValueKind::Int(_) | ValueKind::Float(_))
        && !matches!(date_time.kind(), ValueKind::Bool(_))
    {
        let utc = interp.get_attr(&tz_cls, "utc")?;
        let d = call_method(interp, &dt_cls, "fromtimestamp", &[date_time.clone(), utc])?;
        call_method(interp, &d, "astimezone", &[])?
    } else if matches!(date_time.kind(), ValueKind::Tuple(_))
        || is_instance(
            interp,
            date_time,
            Value::py_class(crate::interpreter::primitive_class_by_name("tuple").expect("tuple")),
        )?
    {
        let gmtoff = if let Ok(v) = interp.get_attr(date_time, "tm_gmtoff")
            && let Some(off) = v.as_int()
        {
            off
        } else {
            let time_mod = interp.load_module("time")?;
            let daylight = interp
                .get_attr(&time_mod, "daylight")?
                .as_int()
                .unwrap_or(0);
            if daylight != 0 {
                let mut dst = py_getitem(interp, date_time, Value::int(8))?
                    .as_int()
                    .unwrap_or(-1);
                if dst == -1 {
                    let mktime = interp.get_attr(&time_mod, "mktime")?;
                    let ts = call_pos(interp, mktime, std::slice::from_ref(date_time))?;
                    let localtime = interp.get_attr(&time_mod, "localtime")?;
                    let lt = call_pos(interp, localtime, &[ts])?;
                    dst = py_getitem(interp, &lt, Value::int(8))?
                        .as_int()
                        .unwrap_or(0);
                }
                let tz_off = if dst != 0 {
                    interp.get_attr(&time_mod, "altzone")?.as_int().unwrap_or(0)
                } else {
                    interp
                        .get_attr(&time_mod, "timezone")?
                        .as_int()
                        .unwrap_or(0)
                };
                -tz_off
            } else {
                -interp
                    .get_attr(&time_mod, "timezone")?
                    .as_int()
                    .unwrap_or(0)
            }
        };
        let delta = call_kw(interp, td_cls, &[], &[("seconds", Value::int(gmtoff))])?;
        let tz = call_pos(interp, tz_cls, &[delta])?;
        let mut args = Vec::with_capacity(6);
        for i in 0..6 {
            args.push(py_getitem(interp, date_time, Value::int(i))?);
        }
        call_kw(interp, dt_cls, &args, &[("tzinfo", tz)])?
    } else if is_instance(interp, date_time, dt_cls)? {
        let tzinfo = interp.get_attr(date_time, "tzinfo")?;
        if tzinfo.is_none() {
            return Err(PyError::named("ValueError", "date_time must be aware"));
        }
        date_time.clone()
    } else if let Some(s) = date_time.as_str() {
        if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
            return Ok(date_time.clone());
        }
        return Err(PyError::named(
            "ValueError",
            "date_time not of a known type",
        ));
    } else {
        return Err(PyError::named(
            "ValueError",
            "date_time not of a known type",
        ));
    };

    let month = interp.get_attr(&dt, "month")?.as_int().unwrap_or(1) as usize;
    let mon_name = MONTHS.get(month).copied().unwrap_or("Jan");
    let fmt = format!("\"%d-{mon_name}-%Y %H:%M:%S %z\"");
    call_method(interp, &dt, "strftime", &[Value::string(fmt)])
}

// ── IMAP4 Wire Parsers ───────────────────────────────────────────────────────

fn normalize_crlf_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' {
            if i + 1 < data.len() && data[i + 1] == b'\n' {
                i += 2;
            } else {
                i += 1;
            }
            out.extend_from_slice(CRLF_BYTES);
        } else if data[i] == b'\n' {
            i += 1;
            out.extend_from_slice(CRLF_BYTES);
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

/// Match `^(?P<tag><tagpre>\d+) (?P<type>[A-Z]+) (?P<data>.*)$`.
fn match_tagged<'a>(tagpre: &[u8], resp: &'a [u8]) -> Option<(&'a [u8], &'a [u8], &'a [u8])> {
    if !resp.starts_with(tagpre) {
        return None;
    }
    let after_pre = &resp[tagpre.len()..];
    let digits_end = after_pre
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(after_pre.len());
    if digits_end == 0 {
        return None;
    }
    let tag_end = tagpre.len() + digits_end;
    if resp.get(tag_end) != Some(&b' ') {
        return None;
    }
    let rest = &resp[tag_end + 1..];
    let type_end = rest
        .iter()
        .position(|b| !b.is_ascii_uppercase())
        .unwrap_or(rest.len());
    if type_end == 0 || rest.get(type_end) != Some(&b' ') {
        return None;
    }
    Some((&resp[..tag_end], &rest[..type_end], &rest[type_end + 1..]))
}

/// Match `^\* (?P<type>[A-Z-]+)( (?P<data>.*))?$`.
fn match_untagged_response(resp: &[u8]) -> Option<(&[u8], Option<&[u8]>)> {
    let rest = resp.strip_prefix(b"* ")?;
    let type_end = rest
        .iter()
        .position(|b| !(b.is_ascii_uppercase() || *b == b'-'))
        .unwrap_or(rest.len());
    if type_end == 0 {
        return None;
    }
    if type_end == rest.len() {
        Some((&rest[..type_end], None))
    } else if rest[type_end] == b' ' {
        Some((&rest[..type_end], Some(&rest[type_end + 1..])))
    } else {
        None
    }
}

type UntaggedStatusMatch<'a> = (&'a [u8], &'a [u8], Option<&'a [u8]>);

/// Match `^\* (?P<data>\d+) (?P<type>[A-Z-]+)( (?P<data2>.*))?$`.
fn match_untagged_status(resp: &[u8]) -> Option<UntaggedStatusMatch<'_>> {
    let rest = resp.strip_prefix(b"* ")?;
    let d_end = rest
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(rest.len());
    if d_end == 0 || rest.get(d_end) != Some(&b' ') {
        return None;
    }
    let after_d = &rest[d_end + 1..];
    let t_end = after_d
        .iter()
        .position(|b| !(b.is_ascii_uppercase() || *b == b'-'))
        .unwrap_or(after_d.len());
    if t_end == 0 {
        return None;
    }
    if t_end == after_d.len() {
        Some((&rest[..d_end], &after_d[..t_end], None))
    } else if after_d[t_end] == b' ' {
        Some((
            &rest[..d_end],
            &after_d[..t_end],
            Some(&after_d[t_end + 1..]),
        ))
    } else {
        None
    }
}

/// Match `^\+( (?P<data>.*))?$`.
fn match_continuation(resp: &[u8]) -> Option<Option<&[u8]>> {
    let rest = resp.strip_prefix(b"+")?;
    if rest.is_empty() {
        Some(None)
    } else if let Some(data) = rest.strip_prefix(b" ") {
        Some(Some(data))
    } else {
        None
    }
}

/// Match `.*{(?P<size>\d+)}$`.
fn match_literal_size(dat: &[u8]) -> Option<usize> {
    let body = dat.strip_suffix(b"}")?;
    let open = body.iter().rposition(|&b| b == b'{')?;
    let digits = &body[open + 1..];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse::<usize>().ok()
}

/// Match `^\[(?P<type>[A-Z-]+)( (?P<data>.*))?\]` (note: greedy up to last `]`).
fn match_response_code(dat: &[u8]) -> Option<(&[u8], Option<&[u8]>)> {
    let rest = dat.strip_prefix(b"[")?;
    let close = rest.iter().rposition(|&b| b == b']')?;
    let inner = &rest[..close];
    let t_end = inner
        .iter()
        .position(|b| !(b.is_ascii_uppercase() || *b == b'-'))
        .unwrap_or(inner.len());
    if t_end == 0 {
        return None;
    }
    if t_end == inner.len() {
        Some((&inner[..t_end], None))
    } else if inner[t_end] == b' ' {
        Some((&inner[..t_end], Some(&inner[t_end + 1..])))
    } else {
        None
    }
}

// ── Shared IMAP4 Core Helpers ────────────────────────────────────────────────

fn imap4_init_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    host: &str,
    port: i64,
    timeout: Value,
) -> Result<()> {
    let dbg = imap_attr(interp, "Debug").unwrap_or_else(|_| Value::int(0));
    interp.assign_attr(self_val.clone(), "debug", dbg)?;
    interp.assign_attr(self_val.clone(), "state", Value::string("LOGOUT"))?;
    interp.assign_attr(self_val.clone(), "literal", Value::none())?;
    interp.assign_attr(
        self_val.clone(),
        "tagged_commands",
        Value::dict(PyDict::default()),
    )?;
    interp.assign_attr(
        self_val.clone(),
        "untagged_responses",
        Value::dict(PyDict::default()),
    )?;
    interp.assign_attr(self_val.clone(), "continuation_response", Value::string(""))?;
    interp.assign_attr(self_val.clone(), "is_readonly", Value::bool_(false))?;
    interp.assign_attr(self_val.clone(), "tagnum", Value::int(0))?;
    interp.assign_attr(self_val.clone(), "_tls_established", Value::bool_(false))?;
    call_method(interp, self_val, "_mode_ascii", &[])?;

    call_method(
        interp,
        self_val,
        "open",
        &[Value::string(host), Value::int(port), timeout],
    )?;

    if let Err(err) = call_method(interp, self_val, "_connect", &[]) {
        let _ = call_method(interp, self_val, "shutdown", &[]);
        return Err(err);
    }
    Ok(())
}

fn imap4_create_socket_impl(
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
    let host_str = get_str_attr(interp, self_val, "host", "")?;
    let host_val = if host_str.is_empty() {
        Value::none()
    } else {
        Value::string(host_str)
    };
    let port = interp.get_attr(self_val, "port")?;
    let address = Value::tuple(vec![host_val, port]);
    let sock_mod = interp.load_module("socket")?;
    let create_conn = interp.get_attr(&sock_mod, "create_connection")?;
    if !timeout.is_none() {
        call_pos(interp, create_conn, &[address, timeout])
    } else {
        call_pos(interp, create_conn, &[address])
    }
}

fn imap4_open_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    host: &str,
    port: i64,
    timeout: Value,
) -> Result<()> {
    interp.assign_attr(self_val.clone(), "host", Value::string(host))?;
    interp.assign_attr(self_val.clone(), "port", Value::int(port))?;
    let sock = call_method(interp, self_val, "_create_socket", &[timeout])?;
    let file = call_method(interp, &sock, "makefile", &[Value::string("rb")])?;
    interp.assign_attr(self_val.clone(), "sock", sock)?;
    interp.assign_attr(self_val.clone(), "file", file)?;
    Ok(())
}

fn make_nested_exc_class(
    name: &str,
    qualname: &str,
    base: Rc<RefCell<PyClass>>,
) -> Rc<RefCell<PyClass>> {
    let mut attrs = IndexMap::new();
    attrs.insert("__module__".to_string(), Value::string("imaplib"));
    attrs.insert("__qualname__".to_string(), Value::string(qualname));
    let cls = PyClass::new(name, qualname, None, attrs);
    let cls_rc = Rc::new(RefCell::new(cls));
    link_class_bases(&cls_rc, vec![base]);
    cls_rc
}

// ── Post-load initialization ─────────────────────────────────────────────────

pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // 1. Build `Commands` dict.
    let mut commands_dict = PyDict::default();
    for &(cmd, states) in COMMANDS_SPEC {
        let tup = Value::tuple(states.iter().map(|&s| Value::string(s)).collect());
        commands_dict.insert(PyKey::str_from(cmd), tup);
    }
    module
        .borrow_mut()
        .insert_attr("Commands".to_string(), Value::dict(commands_dict));

    // 2. Build `Months` and `Mon2num`.
    let months_list = Value::list(MONTHS.iter().map(|&m| Value::string(m)).collect());
    let mut mon2num = PyDict::default();
    for (idx, &m) in MONTHS.iter().enumerate().skip(1) {
        mon2num.insert(
            PyKey::Bytes(Rc::new(m.as_bytes().to_vec())),
            Value::int(idx as i64),
        );
    }
    module
        .borrow_mut()
        .insert_attr("Months".to_string(), months_list);
    module
        .borrow_mut()
        .insert_attr("Mon2num".to_string(), Value::dict(mon2num));

    // 3. Attach nested exception classes `IMAP4.error`, `IMAP4.abort`, `IMAP4.readonly`.
    let exc_base = lookup_exc_class("Exception").expect("Exception must exist");
    let err_cls = make_nested_exc_class("error", "IMAP4.error", exc_base);
    let abort_cls = make_nested_exc_class("abort", "IMAP4.abort", Rc::clone(&err_cls));
    let readonly_cls = make_nested_exc_class("readonly", "IMAP4.readonly", Rc::clone(&abort_cls));

    if let Some(imap4_val) = module.borrow().attrs.get("IMAP4").cloned()
        && let ValueKind::PyClass(imap4_rc) = imap4_val.kind()
    {
        let mut borrowed = imap4_rc.borrow_mut();
        borrowed
            .attrs
            .insert("error".to_string(), Value::py_class(err_cls));
        borrowed
            .attrs
            .insert("abort".to_string(), Value::py_class(abort_cls));
        borrowed
            .attrs
            .insert("readonly".to_string(), Value::py_class(readonly_cls));
    }

    Ok(())
}

// ── Native module definition ─────────────────────────────────────────────────

pyrust_module! {
    constants {
        "__version__" => Value::string("2.58"),
        "__all__" => Value::list(vec![
            Value::string("IMAP4"),
            Value::string("IMAP4_stream"),
            Value::string("Internaldate2tuple"),
            Value::string("Int2AP"),
            Value::string("ParseFlags"),
            Value::string("Time2Internaldate"),
            Value::string("IMAP4_SSL"),
        ]),
        "CRLF" => Value::bytes(CRLF_BYTES.to_vec()),
        "Debug" => Value::int(0),
        "IMAP4_PORT" => Value::int(IMAP4_PORT_VAL),
        "IMAP4_SSL_PORT" => Value::int(IMAP4_SSL_PORT_VAL),
        "AllowedVersions" => Value::tuple(vec![
            Value::string("IMAP4REV1"),
            Value::string("IMAP4"),
        ]),
        "_MAXLINE" => Value::int(MAXLINE_VAL),
        "HAVE_SSL" => Value::bool_(true),
    }

    // ── Module-level helper functions ────────────────────────────────────────

    #[py_name = "Int2AP"]
    fn int2ap(num: PyValue) -> Result<Value> {
        let abs_v = call_pos(_interp, Value::builtin_function("abs"), &[num.0])?;
        let int_v = call_pos(_interp, Value::builtin_function("int"), &[abs_v])?;
        let n = int_v.as_int().ok_or_else(|| {
            PyError::named("OverflowError", "Int2AP integer too large")
        })?;
        Ok(Value::bytes(int2ap_impl(n)))
    }

    #[py_name = "ParseFlags"]
    fn parse_flags(resp: PyBytesLike<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::tuple(parse_flags_impl(resp.as_bytes())))
    }

    #[py_name = "Internaldate2tuple"]
    fn internaldate2tuple(resp: PyBytesLike<'_>) -> Result<Value> {
        internaldate2tuple_impl(_interp, resp.as_bytes())
    }

    #[py_name = "Time2Internaldate"]
    fn time2internaldate(date_time: PyValue) -> Result<Value> {
        time2internaldate_impl(_interp, &date_time.0)
    }

    // ── _Authenticator ───────────────────────────────────────────────────────

    class _Authenticator {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            mechinst: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "mech", mechinst.0)?;
            Ok(Value::none())
        }

        fn process(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyValue,
        ) -> Result<Value> {
            let decoded = call_method(_interp, self_val.0, "decode", &[data.0])?;
            let mech = _interp.get_attr(self_val.0, "mech")?;
            let ret = call_pos(_interp, mech, &[decoded])?;
            if ret.is_none() {
                return Ok(Value::bytes(b"*".to_vec()));
            }
            call_method(_interp, self_val.0, "encode", &[ret])
        }

        fn encode(#[positional_only] _self: PyValueRef<'_>, inp: PyValue) -> Result<Value> {
            let raw = if let Some(s) = inp.0.as_str() {
                s.as_bytes().to_vec()
            } else if let ValueKind::Bytes(b) = inp.0.kind() {
                b.as_ref().clone()
            } else if let Some(ba) = pyrust_builtins::bytearray::as_bytearray_snapshot(&inp.0) {
                ba
            } else {
                let s = value_to_str(_interp, &inp.0)?;
                s.into_bytes()
            };
            Ok(Value::bytes(b64_encode(&raw)))
        }

        fn decode(#[positional_only] _self: PyValueRef<'_>, inp: PyValue) -> Result<Value> {
            let _ = _interp;
            let raw = match inp.0.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                ValueKind::Str(s) => s.as_bytes().to_vec(),
                _ => Vec::new(),
            };
            if raw.is_empty() {
                return Ok(Value::bytes(Vec::new()));
            }
            Ok(Value::bytes(b64_decode(&raw)))
        }
    }

    // ── IMAP4 ────────────────────────────────────────────────────────────────

    class IMAP4 {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] host: PyStr<'_>,
            #[default(IMAP4_PORT_VAL.into())] port: PyIndex,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let timeout_val = timeout.as_value().cloned().unwrap_or_else(Value::none);
            imap4_init_impl(_interp, self_val.0, &host.0, port_i64, timeout_val)?;
            Ok(Value::none())
        }

        fn _mode_ascii(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "utf8_enabled", Value::bool_(false))?;
            _interp.assign_attr(self_val.0.clone(), "_encoding", Value::string("ascii"))?;
            Ok(Value::none())
        }

        fn _mode_utf8(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "utf8_enabled", Value::bool_(true))?;
            _interp.assign_attr(self_val.0.clone(), "_encoding", Value::string("utf-8"))?;
            Ok(Value::none())
        }

        fn _connect(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let random_mod = _interp.load_module("random")?;
            let randint = _interp.get_attr(&random_mod, "randint")?;
            let rv = call_pos(_interp, randint, &[Value::int(4096), Value::int(65535)])?;
            let tagpre = int2ap_impl(rv.as_int().unwrap_or(4096));
            _interp.assign_attr(self_val.0.clone(), "tagpre", Value::bytes(tagpre))?;
            _interp.assign_attr(self_val.0.clone(), "_cmd_log_len", Value::int(10))?;
            _interp.assign_attr(self_val.0.clone(), "_cmd_log_idx", Value::int(0))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_cmd_log",
                Value::dict(PyDict::default()),
            )?;

            let welcome = call_method(_interp, self_val.0, "_get_response", &[])?;
            _interp.assign_attr(self_val.0.clone(), "welcome", welcome.clone())?;

            let ur = _interp.get_attr(self_val.0, "untagged_responses")?;
            let has_preauth = ur
                .as_dict()
                .is_some_and(|d| d.contains_key(&PyKey::str_from("PREAUTH")));
            let has_ok = ur
                .as_dict()
                .is_some_and(|d| d.contains_key(&PyKey::str_from("OK")));
            if has_preauth {
                _interp.assign_attr(self_val.0.clone(), "state", Value::string("AUTH"))?;
            } else if has_ok {
                _interp.assign_attr(self_val.0.clone(), "state", Value::string("NONAUTH"))?;
            } else {
                return Err(raise_self_exc(_interp, self_val.0, "error", welcome));
            }

            call_method(_interp, self_val.0, "_get_capabilities", &[])?;
            let caps_val = _interp.get_attr(self_val.0, "capabilities")?;
            let caps = _interp.collect_iterable(&caps_val)?;
            let allowed_val = imap_attr(_interp, "AllowedVersions")?;
            let allowed = _interp.collect_iterable(&allowed_val)?;
            for ver in allowed {
                let ver_s = value_to_str(_interp, &ver)?;
                if caps.iter().any(|c| c.as_str() == Some(ver_s.as_str())) {
                    _interp.assign_attr(
                        self_val.0.clone(),
                        "PROTOCOL_VERSION",
                        Value::string(ver_s),
                    )?;
                    return Ok(Value::none());
                }
            }
            Err(raise_self_exc(
                _interp,
                self_val.0,
                "error",
                Value::string("server not IMAP4 compliant"),
            ))
        }

        fn __getattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            attr: PyStr<'_>,
        ) -> Result<Value> {
            let commands = imap_attr(_interp, "Commands")?;
            let in_cmds = commands
                .as_dict()
                .is_some_and(|d| d.contains_key(&PyKey::str_from(&attr.0)));
            if in_cmds {
                let lower = attr.0.to_ascii_lowercase();
                return _interp.get_attr(self_val.0, &lower);
            }
            Err(PyError::named(
                "AttributeError",
                format!("Unknown IMAP4 command: '{}'", attr.0),
            ))
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            let state = get_str_attr(_interp, self_val.0, "state", "LOGOUT")?;
            if state == "LOGOUT" {
                return Ok(Value::none());
            }
            match call_method(_interp, self_val.0, "logout", &[]) {
                Ok(_) => Ok(Value::none()),
                Err(err) if is_os_error(_interp, &err) => Ok(Value::none()),
                Err(err) => Err(err),
            }
        }

        // ── Overridable transport methods ────────────────────────────────────

        fn _create_socket(
            #[positional_only] self_val: PyValueRef<'_>,
            timeout: PyValue,
        ) -> Result<Value> {
            imap4_create_socket_impl(_interp, self_val.0, timeout.0)
        }

        fn open(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] host: PyStr<'_>,
            #[default(IMAP4_PORT_VAL.into())] port: PyIndex,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let timeout_val = timeout.as_value().cloned().unwrap_or_else(Value::none);
            imap4_open_impl(_interp, self_val.0, &host.0, port_i64, timeout_val)?;
            Ok(Value::none())
        }

        fn read(#[positional_only] self_val: PyValueRef<'_>, size: PyIndex) -> Result<Value> {
            let n = size.expect_i64(FN_NAME, "size")?;
            let file = _interp.get_attr(self_val.0, "file")?;
            call_method(_interp, &file, "read", &[Value::int(n)])
        }

        fn readline(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let file = _interp.get_attr(self_val.0, "file")?;
            let line = call_method(_interp, &file, "readline", &[Value::int(MAXLINE_VAL + 1)])?;
            if let ValueKind::Bytes(b) = line.kind()
                && (b.len() as i64) > MAXLINE_VAL
            {
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "error",
                    Value::string(format!("got more than {MAXLINE_VAL} bytes")),
                ));
            }
            Ok(line)
        }

        fn send(#[positional_only] self_val: PyValueRef<'_>, data: PyValue) -> Result<Value> {
            let sock = _interp.get_attr(self_val.0, "sock")?;
            call_method(_interp, &sock, "sendall", &[data.0])?;
            Ok(Value::none())
        }

        fn shutdown(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let file = _interp.get_attr(self_val.0, "file")?;
            if !file.is_none() {
                let _ = call_method(_interp, &file, "close", &[]);
            }
            let sock = _interp.get_attr(self_val.0, "sock")?;
            if !sock.is_none() {
                let sock_mod = _interp.load_module("socket")?;
                let shut_rdwr = _interp.get_attr(&sock_mod, "SHUT_RDWR")?;
                let _ = call_method(_interp, &sock, "shutdown", &[shut_rdwr]);
                call_method(_interp, &sock, "close", &[])?;
            }
            Ok(Value::none())
        }

        fn socket(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "sock")
        }

        // ── Utility methods ──────────────────────────────────────────────────

        fn recent(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[
                    Value::string("OK"),
                    Value::list(vec![Value::none()]),
                    Value::string("RECENT"),
                ],
            )?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            let last = py_getitem(_interp, &dat, Value::int(-1))?;
            if _interp.truthy_value(&last)? {
                return Ok(pair);
            }
            let noop_pair = call_method(_interp, self_val.0, "noop", &[])?;
            let typ = py_getitem(_interp, &noop_pair, Value::int(0))?;
            let ndat = py_getitem(_interp, &noop_pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, ndat, Value::string("RECENT")],
            )
        }

        fn response(
            #[positional_only] self_val: PyValueRef<'_>,
            code: PyStr<'_>,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[
                    Value::string(&*code.0),
                    Value::list(vec![Value::none()]),
                    Value::string(code.0.to_ascii_uppercase()),
                ],
            )
        }

        // ── IMAP4 Commands ───────────────────────────────────────────────────

        fn append(
            #[positional_only] self_val: PyValueRef<'_>,
            mailbox: PyValue,
            flags: PyValue,
            date_time: PyValue,
            message: PyBytesLike<'_>,
        ) -> Result<Value> {
            let mbox = if _interp.truthy_value(&mailbox.0)? {
                mailbox.0
            } else {
                Value::string("INBOX")
            };
            let flags_val = if _interp.truthy_value(&flags.0)? {
                let fs = value_to_str(_interp, &flags.0)?;
                if fs.starts_with('(') && fs.ends_with(')') {
                    Value::string(fs)
                } else {
                    Value::string(format!("({fs})"))
                }
            } else {
                Value::none()
            };
            let dt_val = if _interp.truthy_value(&date_time.0)? {
                time2internaldate_impl(_interp, &date_time.0)?
            } else {
                Value::none()
            };
            let mut literal = normalize_crlf_bytes(message.as_bytes());
            if get_bool_attr(_interp, self_val.0, "utf8_enabled")? {
                let mut wrapped = Vec::with_capacity(literal.len() + 7);
                wrapped.extend_from_slice(b"UTF8 (");
                wrapped.extend_from_slice(&literal);
                wrapped.push(b')');
                literal = wrapped;
            }
            _interp.assign_attr(self_val.0.clone(), "literal", Value::bytes(literal))?;
            call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("APPEND"), mbox, flags_val, dt_val],
            )
        }

        fn authenticate(
            #[positional_only] self_val: PyValueRef<'_>,
            mechanism: PyStr<'_>,
            authobject: PyValue,
        ) -> Result<Value> {
            let mech = mechanism.0.to_ascii_uppercase();
            let auth_cls = imap_attr(_interp, "_Authenticator")?;
            let auth_inst = call_pos(_interp, auth_cls, &[authobject.0])?;
            let proc_method = _interp.get_attr(&auth_inst, "process")?;
            _interp.assign_attr(self_val.0.clone(), "literal", proc_method)?;
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("AUTHENTICATE"), Value::string(mech)],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            if typ.as_str() != Some("OK") {
                let last = py_getitem(_interp, &dat, Value::int(-1))?;
                let msg = match last.kind() {
                    ValueKind::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                    _ => value_to_str(_interp, &last)?,
                };
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "error",
                    Value::string(msg),
                ));
            }
            _interp.assign_attr(self_val.0.clone(), "state", Value::string("AUTH"))?;
            Ok(pair)
        }

        fn capability(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("CAPABILITY")],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("CAPABILITY")],
            )
        }

        fn check(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "_simple_command", &[Value::string("CHECK")])
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let res = call_method(_interp, self_val.0, "_simple_command", &[Value::string("CLOSE")]);
            _interp.assign_attr(self_val.0.clone(), "state", Value::string("AUTH"))?;
            res
        }

        fn copy(
            #[positional_only] self_val: PyValueRef<'_>,
            message_set: PyValue,
            new_mailbox: PyValue,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("COPY"), message_set.0, new_mailbox.0],
            )
        }

        fn create(#[positional_only] self_val: PyValueRef<'_>, mailbox: PyValue) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("CREATE"), mailbox.0],
            )
        }

        fn delete(#[positional_only] self_val: PyValueRef<'_>, mailbox: PyValue) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("DELETE"), mailbox.0],
            )
        }

        fn deleteacl(
            #[positional_only] self_val: PyValueRef<'_>,
            mailbox: PyValue,
            who: PyValue,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("DELETEACL"), mailbox.0, who.0],
            )
        }

        fn enable(
            #[positional_only] self_val: PyValueRef<'_>,
            capability: PyStr<'_>,
        ) -> Result<Value> {
            let caps_val = _interp.get_attr(self_val.0, "capabilities")?;
            let caps = _interp.collect_iterable(&caps_val)?;
            if !caps.iter().any(|c| c.as_str() == Some("ENABLE")) {
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "error",
                    Value::string("Server does not support ENABLE"),
                ));
            }
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("ENABLE"), Value::string(&*capability.0)],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            if typ.as_str() == Some("OK")
                && capability.0.to_ascii_uppercase().contains("UTF8=ACCEPT")
            {
                call_method(_interp, self_val.0, "_mode_utf8", &[])?;
            }
            Ok(pair)
        }

        fn expunge(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("EXPUNGE")],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("EXPUNGE")],
            )
        }

        fn fetch(
            #[positional_only] self_val: PyValueRef<'_>,
            message_set: PyValue,
            message_parts: PyValue,
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("FETCH"), message_set.0, message_parts.0],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("FETCH")],
            )
        }

        fn getacl(#[positional_only] self_val: PyValueRef<'_>, mailbox: PyValue) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("GETACL"), mailbox.0],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("ACL")],
            )
        }

        fn getannotation(
            #[positional_only] self_val: PyValueRef<'_>,
            mailbox: PyValue,
            entry: PyValue,
            attribute: PyValue,
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[
                    Value::string("GETANNOTATION"),
                    mailbox.0,
                    entry.0,
                    attribute.0,
                ],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("ANNOTATION")],
            )
        }

        fn getquota(#[positional_only] self_val: PyValueRef<'_>, root: PyValue) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("GETQUOTA"), root.0],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("QUOTA")],
            )
        }

        fn getquotaroot(
            #[positional_only] self_val: PyValueRef<'_>,
            mailbox: PyValue,
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("GETQUOTAROOT"), mailbox.0],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            let q_pair = call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ.clone(), dat.clone(), Value::string("QUOTA")],
            )?;
            let quota = py_getitem(_interp, &q_pair, Value::int(1))?;
            let qr_pair = call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("QUOTAROOT")],
            )?;
            let typ_final = py_getitem(_interp, &qr_pair, Value::int(0))?;
            let quotaroot = py_getitem(_interp, &qr_pair, Value::int(1))?;
            Ok(Value::tuple(vec![
                typ_final,
                Value::list(vec![quotaroot, quota]),
            ]))
        }

        fn list(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("\"\"".into())] directory: PyStr<'_>,
            #[default("*".into())] pattern: PyStr<'_>,
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[
                    Value::string("LIST"),
                    Value::string(&*directory.0),
                    Value::string(&*pattern.0),
                ],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("LIST")],
            )
        }

        fn login(
            #[positional_only] self_val: PyValueRef<'_>,
            user: PyStr<'_>,
            password: PyStr<'_>,
        ) -> Result<Value> {
            let quoted_pw =
                call_method(_interp, self_val.0, "_quote", &[Value::string(&*password.0)])?;
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("LOGIN"), Value::string(&*user.0), quoted_pw],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            if typ.as_str() != Some("OK") {
                let last = py_getitem(_interp, &dat, Value::int(-1))?;
                return Err(raise_self_exc(_interp, self_val.0, "error", last));
            }
            _interp.assign_attr(self_val.0.clone(), "state", Value::string("AUTH"))?;
            Ok(pair)
        }

        fn login_cram_md5(
            #[positional_only] self_val: PyValueRef<'_>,
            user: PyValue,
            password: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "user", user.0)?;
            _interp.assign_attr(self_val.0.clone(), "password", password.0)?;
            let auth_fn = _interp.get_attr(self_val.0, "_CRAM_MD5_AUTH")?;
            call_method(
                _interp,
                self_val.0,
                "authenticate",
                &[Value::string("CRAM-MD5"), auth_fn],
            )
        }

        #[py_name = "_CRAM_MD5_AUTH"]
        fn cram_md5_auth(
            #[positional_only] self_val: PyValueRef<'_>,
            challenge: PyValue,
        ) -> Result<Value> {
            let user = get_str_attr(_interp, self_val.0, "user", "")?;
            let pw_val = _interp.get_attr(self_val.0, "password")?;
            let pwd = if let Some(s) = pw_val.as_str() {
                Value::bytes(s.as_bytes().to_vec())
            } else {
                pw_val
            };
            let hmac_mod = _interp.load_module("hmac")?;
            let hmac_cls = _interp.get_attr(&hmac_mod, "HMAC")?;
            let h = call_pos(_interp, hmac_cls, &[pwd, challenge.0, Value::string("md5")])?;
            let hex_val = call_method(_interp, &h, "hexdigest", &[])?;
            let hex = value_to_str(_interp, &hex_val)?;
            Ok(Value::string(format!("{user} {hex}")))
        }

        fn logout(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "state", Value::string("LOGOUT"))?;
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("LOGOUT")],
            )?;
            call_method(_interp, self_val.0, "shutdown", &[])?;
            Ok(pair)
        }

        fn lsub(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("\"\"".into())] directory: PyStr<'_>,
            #[default("*".into())] pattern: PyStr<'_>,
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[
                    Value::string("LSUB"),
                    Value::string(&*directory.0),
                    Value::string(&*pattern.0),
                ],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("LSUB")],
            )
        }

        fn myrights(
            #[positional_only] self_val: PyValueRef<'_>,
            mailbox: PyValue,
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("MYRIGHTS"), mailbox.0],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("MYRIGHTS")],
            )
        }

        fn namespace(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("NAMESPACE")],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("NAMESPACE")],
            )
        }

        fn noop(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "_simple_command", &[Value::string("NOOP")])
        }

        fn partial(
            #[positional_only] self_val: PyValueRef<'_>,
            message_num: PyValue,
            message_part: PyValue,
            start: PyValue,
            length: PyValue,
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[
                    Value::string("PARTIAL"),
                    message_num.0,
                    message_part.0,
                    start.0,
                    length.0,
                ],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("FETCH")],
            )
        }

        fn proxyauth(#[positional_only] self_val: PyValueRef<'_>, user: PyValue) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("PROXYAUTH"), user.0],
            )
        }

        fn rename(
            #[positional_only] self_val: PyValueRef<'_>,
            oldmailbox: PyValue,
            newmailbox: PyValue,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("RENAME"), oldmailbox.0, newmailbox.0],
            )
        }

        fn search(
            #[positional_only] self_val: PyValueRef<'_>,
            charset: PyValue,
            #[varargs] criteria: &[Value],
        ) -> Result<Value> {
            let mut cmd_args = Vec::with_capacity(criteria.len() + 3);
            cmd_args.push(Value::string("SEARCH"));
            if _interp.truthy_value(&charset.0)? {
                if get_bool_attr(_interp, self_val.0, "utf8_enabled")? {
                    return Err(raise_self_exc(
                        _interp,
                        self_val.0,
                        "error",
                        Value::string("Non-None charset not valid in UTF8 mode"),
                    ));
                }
                cmd_args.push(Value::string("CHARSET"));
                cmd_args.push(charset.0);
            }
            cmd_args.extend_from_slice(criteria);
            let pair = call_method(_interp, self_val.0, "_simple_command", &cmd_args)?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("SEARCH")],
            )
        }

        fn select(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("INBOX".into())] mailbox: PyStr<'_>,
            #[default(false.into())] readonly: PyBool,
        ) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "untagged_responses",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "is_readonly", Value::bool_(readonly.0))?;
            let name = if readonly.0 { "EXAMINE" } else { "SELECT" };
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string(name), Value::string(&*mailbox.0)],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            if typ.as_str() != Some("OK") {
                _interp.assign_attr(self_val.0.clone(), "state", Value::string("AUTH"))?;
                return Ok(pair);
            }
            _interp.assign_attr(self_val.0.clone(), "state", Value::string("SELECTED"))?;
            let ur = _interp.get_attr(self_val.0, "untagged_responses")?;
            let is_ro = ur
                .as_dict()
                .is_some_and(|d| d.contains_key(&PyKey::str_from("READ-ONLY")));
            if is_ro && !readonly.0 {
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "readonly",
                    Value::string(format!("{} is not writable", mailbox.0)),
                ));
            }
            let exists = ur
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from("EXISTS")).cloned())
                .unwrap_or_else(|| Value::list(vec![Value::none()]));
            Ok(Value::tuple(vec![typ, exists]))
        }

        fn setacl(
            #[positional_only] self_val: PyValueRef<'_>,
            mailbox: PyValue,
            who: PyValue,
            what: PyValue,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("SETACL"), mailbox.0, who.0, what.0],
            )
        }

        fn setannotation(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let mut cmd_args = Vec::with_capacity(args.len() + 1);
            cmd_args.push(Value::string("SETANNOTATION"));
            cmd_args.extend_from_slice(args);
            let pair = call_method(_interp, self_val.0, "_simple_command", &cmd_args)?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("ANNOTATION")],
            )
        }

        fn setquota(
            #[positional_only] self_val: PyValueRef<'_>,
            root: PyValue,
            limits: PyValue,
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("SETQUOTA"), root.0, limits.0],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("QUOTA")],
            )
        }

        fn sort(
            #[positional_only] self_val: PyValueRef<'_>,
            sort_criteria: PyStr<'_>,
            charset: PyValue,
            #[varargs] search_criteria: &[Value],
        ) -> Result<Value> {
            let sc = if sort_criteria.0.starts_with('(') && sort_criteria.0.ends_with(')') {
                sort_criteria.0.to_string()
            } else {
                format!("({})", sort_criteria.0)
            };
            let mut cmd_args = Vec::with_capacity(search_criteria.len() + 3);
            cmd_args.push(Value::string("SORT"));
            cmd_args.push(Value::string(sc));
            cmd_args.push(charset.0);
            cmd_args.extend_from_slice(search_criteria);
            let pair = call_method(_interp, self_val.0, "_simple_command", &cmd_args)?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("SORT")],
            )
        }

        fn starttls(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] ssl_context: OptValueRef<'_>,
        ) -> Result<Value> {
            if get_bool_attr(_interp, self_val.0, "_tls_established")? {
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "abort",
                    Value::string("TLS session already established"),
                ));
            }
            let caps_val = _interp.get_attr(self_val.0, "capabilities")?;
            let caps = _interp.collect_iterable(&caps_val)?;
            if !caps.iter().any(|c| c.as_str() == Some("STARTTLS")) {
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "abort",
                    Value::string("TLS not supported by server"),
                ));
            }
            let ctx = match ssl_context.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let ssl_mod = _interp.load_module("ssl")?;
                    let mk = _interp.get_attr(&ssl_mod, "_create_stdlib_context")?;
                    call_pos(_interp, mk, &[])?
                }
            };
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("STARTTLS")],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            if typ.as_str() == Some("OK") {
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
                call_method(_interp, self_val.0, "_get_capabilities", &[])?;
            } else {
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "error",
                    Value::string("Couldn't establish TLS session"),
                ));
            }
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("STARTTLS")],
            )
        }

        fn status(
            #[positional_only] self_val: PyValueRef<'_>,
            mailbox: PyValue,
            names: PyValue,
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("STATUS"), mailbox.0, names.0],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("STATUS")],
            )
        }

        fn store(
            #[positional_only] self_val: PyValueRef<'_>,
            message_set: PyValue,
            command: PyValue,
            flags: PyStr<'_>,
        ) -> Result<Value> {
            let fl = if flags.0.starts_with('(') && flags.0.ends_with(')') {
                flags.0.to_string()
            } else {
                format!("({})", flags.0)
            };
            let pair = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[
                    Value::string("STORE"),
                    message_set.0,
                    command.0,
                    Value::string(fl),
                ],
            )?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("FETCH")],
            )
        }

        fn subscribe(
            #[positional_only] self_val: PyValueRef<'_>,
            mailbox: PyValue,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("SUBSCRIBE"), mailbox.0],
            )
        }

        fn thread(
            #[positional_only] self_val: PyValueRef<'_>,
            threading_algorithm: PyValue,
            charset: PyValue,
            #[varargs] search_criteria: &[Value],
        ) -> Result<Value> {
            let mut cmd_args = Vec::with_capacity(search_criteria.len() + 3);
            cmd_args.push(Value::string("THREAD"));
            cmd_args.push(threading_algorithm.0);
            cmd_args.push(charset.0);
            cmd_args.extend_from_slice(search_criteria);
            let pair = call_method(_interp, self_val.0, "_simple_command", &cmd_args)?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string("THREAD")],
            )
        }

        fn uid(
            #[positional_only] self_val: PyValueRef<'_>,
            command: PyStr<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let cmd = command.0.to_ascii_uppercase();
            let commands = imap_attr(_interp, "Commands")?;
            let allowed_opt = commands
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(&cmd)).cloned());
            let Some(allowed_val) = allowed_opt else {
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "error",
                    Value::string(format!("Unknown IMAP4 UID command: {cmd}")),
                ));
            };
            let state = get_str_attr(_interp, self_val.0, "state", "LOGOUT")?;
            let allowed_states = _interp.collect_iterable(&allowed_val)?;
            if !allowed_states.iter().any(|s| s.as_str() == Some(state.as_str())) {
                let state_names: Vec<String> = allowed_states
                    .iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect();
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "error",
                    Value::string(format!(
                        "command {cmd} illegal in state {state}, only allowed in states {}",
                        state_names.join(", ")
                    )),
                ));
            }
            let mut cmd_args = Vec::with_capacity(args.len() + 2);
            cmd_args.push(Value::string("UID"));
            cmd_args.push(Value::string(&cmd));
            cmd_args.extend_from_slice(args);
            let pair = call_method(_interp, self_val.0, "_simple_command", &cmd_args)?;
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            let resp_name = if matches!(cmd.as_str(), "SEARCH" | "SORT" | "THREAD") {
                cmd.as_str()
            } else {
                "FETCH"
            };
            call_method(
                _interp,
                self_val.0,
                "_untagged_response",
                &[typ, dat, Value::string(resp_name)],
            )
        }

        fn unsubscribe(
            #[positional_only] self_val: PyValueRef<'_>,
            mailbox: PyValue,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("UNSUBSCRIBE"), mailbox.0],
            )
        }

        fn unselect(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let res = call_method(
                _interp,
                self_val.0,
                "_simple_command",
                &[Value::string("UNSELECT")],
            );
            _interp.assign_attr(self_val.0.clone(), "state", Value::string("AUTH"))?;
            res
        }

        fn xatom(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let up = name.0.to_ascii_uppercase();
            let commands = imap_attr(_interp, "Commands")?;
            let exists = commands
                .as_dict()
                .is_some_and(|d| d.contains_key(&PyKey::str_from(&up)));
            if !exists {
                let state = _interp.get_attr(self_val.0, "state")?;
                commands.dict_insert(PyKey::str_from(&up), Value::tuple(vec![state]))?;
            }
            let mut cmd_args = Vec::with_capacity(args.len() + 1);
            cmd_args.push(Value::string(up));
            cmd_args.extend_from_slice(args);
            call_method(_interp, self_val.0, "_simple_command", &cmd_args)
        }

        // ── Private protocol machinery ───────────────────────────────────────

        fn _append_untagged(
            #[positional_only] self_val: PyValueRef<'_>,
            typ: PyStr<'_>,
            dat: PyValue,
        ) -> Result<Value> {
            let val = if dat.0.is_none() {
                Value::bytes(Vec::new())
            } else {
                dat.0
            };
            let ur = _interp.get_attr(self_val.0, "untagged_responses")?;
            let key = PyKey::str_from(&typ.0);
            let existing = ur.as_dict().and_then(|d| d.get(&key).cloned());
            if let Some(lst) = existing {
                lst.list_push(val)?;
            } else {
                ur.dict_insert(key, Value::list(vec![val]))?;
            }
            Ok(Value::none())
        }

        fn _check_bye(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ur = _interp.get_attr(self_val.0, "untagged_responses")?;
            let bye_opt = ur
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from("BYE")).cloned());
            if let Some(bye_list) = bye_opt
                && _interp.truthy_value(&bye_list)?
            {
                let last = py_getitem(_interp, &bye_list, Value::int(-1))?;
                let enc = get_str_attr(_interp, self_val.0, "_encoding", "ascii")?;
                let decoded = match last.kind() {
                    ValueKind::Bytes(_) => call_method(
                        _interp,
                        &last,
                        "decode",
                        &[Value::string(enc), Value::string("replace")],
                    )?,
                    _ => Value::string(value_to_str(_interp, &last)?),
                };
                return Err(raise_self_exc(_interp, self_val.0, "abort", decoded));
            }
            Ok(Value::none())
        }

        fn _command(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let commands = imap_attr(_interp, "Commands")?;
            let allowed_val = commands
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(&name.0)).cloned())
                .ok_or_else(|| {
                    PyError::named("KeyError", format!("'{}'", name.0))
                })?;
            let state = get_str_attr(_interp, self_val.0, "state", "LOGOUT")?;
            let allowed_states = _interp.collect_iterable(&allowed_val)?;
            if !allowed_states.iter().any(|s| s.as_str() == Some(state.as_str())) {
                _interp.assign_attr(self_val.0.clone(), "literal", Value::none())?;
                let names: Vec<String> = allowed_states
                    .iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect();
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "error",
                    Value::string(format!(
                        "command {} illegal in state {state}, only allowed in states {}",
                        name.0,
                        names.join(", ")
                    )),
                ));
            }

            let ur = _interp.get_attr(self_val.0, "untagged_responses")?;
            for t in ["OK", "NO", "BAD"] {
                let _ = call_method(_interp, &ur, "pop", &[Value::string(t), Value::none()]);
            }
            let has_ro = ur
                .as_dict()
                .is_some_and(|d| d.contains_key(&PyKey::str_from("READ-ONLY")));
            if has_ro && !get_bool_attr(_interp, self_val.0, "is_readonly")? {
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "readonly",
                    Value::string("mailbox status changed to READ-ONLY"),
                ));
            }

            let tag = call_method(_interp, self_val.0, "_new_tag", &[])?;
            let tag_bytes = match tag.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => Vec::new(),
            };
            let enc = get_str_attr(_interp, self_val.0, "_encoding", "ascii")?;
            let name_bytes_val = call_method(
                _interp,
                &Value::string(&*name.0),
                "encode",
                &[Value::string(&enc)],
            )?;
            let mut data = Vec::new();
            data.extend_from_slice(&tag_bytes);
            data.push(b' ');
            if let ValueKind::Bytes(nb) = name_bytes_val.kind() {
                data.extend_from_slice(nb);
            }
            for arg in args {
                if arg.is_none() {
                    continue;
                }
                let ab = if arg.as_str().is_some() {
                    let ev = call_method(_interp, arg, "encode", &[Value::string(&enc)])?;
                    match ev.kind() {
                        ValueKind::Bytes(b) => b.as_ref().clone(),
                        _ => Vec::new(),
                    }
                } else if let ValueKind::Bytes(b) = arg.kind() {
                    b.as_ref().clone()
                } else {
                    let sv = value_to_str(_interp, arg)?;
                    let ev = call_method(
                        _interp,
                        &Value::string(sv),
                        "encode",
                        &[Value::string(&enc)],
                    )?;
                    match ev.kind() {
                        ValueKind::Bytes(b) => b.as_ref().clone(),
                        _ => Vec::new(),
                    }
                };
                data.push(b' ');
                data.extend_from_slice(&ab);
            }

            let literal = _interp.get_attr(self_val.0, "literal")?;
            let literator: Option<Value>;
            let mut literal_bytes: Option<Value> = None;
            if !literal.is_none() {
                _interp.assign_attr(self_val.0.clone(), "literal", Value::none())?;
                let is_call = call_pos(
                    _interp,
                    Value::builtin_function("callable"),
                    std::slice::from_ref(&literal),
                )?;
                if _interp.truthy_value(&is_call)? {
                    literator = Some(literal);
                } else {
                    literator = None;
                    let lit_len = match literal.kind() {
                        ValueKind::Bytes(b) => b.len(),
                        _ => 0,
                    };
                    let suffix = format!(" {{{lit_len}}}");
                    data.extend_from_slice(suffix.as_bytes());
                    literal_bytes = Some(literal);
                }
            } else {
                literator = None;
            }

            data.extend_from_slice(CRLF_BYTES);
            if let Err(err) = call_method(_interp, self_val.0, "send", &[Value::bytes(data)]) {
                if is_os_error(_interp, &err) {
                    return Err(raise_self_exc(
                        _interp,
                        self_val.0,
                        "abort",
                        Value::string(format!("socket error: {}", err_message(&err))),
                    ));
                }
                return Err(err);
            }

            if literator.is_none() && literal_bytes.is_none() {
                return Ok(tag);
            }

            loop {
                // Wait for continuation response.
                loop {
                    let r = call_method(_interp, self_val.0, "_get_response", &[])?;
                    if r.is_none() {
                        break;
                    }
                    let tc = _interp.get_attr(self_val.0, "tagged_commands")?;
                    let cur = py_getitem(_interp, &tc, tag.clone())?;
                    if _interp.truthy_value(&cur)? {
                        return Ok(tag);
                    }
                }

                let to_send = if let Some(ref lit_fn) = literator {
                    let cont = _interp.get_attr(self_val.0, "continuation_response")?;
                    call_pos(_interp, lit_fn.clone(), &[cont])?
                } else {
                    literal_bytes.take().unwrap_or_else(|| Value::bytes(Vec::new()))
                };

                if let Err(err) = call_method(_interp, self_val.0, "send", &[to_send]) {
                    if is_os_error(_interp, &err) {
                        return Err(raise_self_exc(
                            _interp,
                            self_val.0,
                            "abort",
                            Value::string(format!("socket error: {}", err_message(&err))),
                        ));
                    }
                    return Err(err);
                }
                if let Err(err) = call_method(
                    _interp,
                    self_val.0,
                    "send",
                    &[Value::bytes(CRLF_BYTES.to_vec())],
                ) {
                    if is_os_error(_interp, &err) {
                        return Err(raise_self_exc(
                            _interp,
                            self_val.0,
                            "abort",
                            Value::string(format!("socket error: {}", err_message(&err))),
                        ));
                    }
                    return Err(err);
                }

                if literator.is_none() {
                    break;
                }
            }

            Ok(tag)
        }

        fn _command_complete(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            tag: PyValue,
        ) -> Result<Value> {
            let logout = name.0.as_ref() == "LOGOUT";
            if !logout {
                call_method(_interp, self_val.0, "_check_bye", &[])?;
            }
            let pair = match call_method_kw(
                _interp,
                self_val.0,
                "_get_tagged_response",
                &[tag.0],
                &[("expect_bye", Value::bool_(logout))],
            ) {
                Ok(p) => p,
                Err(err) => {
                    let abort_cls = _interp.get_attr(self_val.0, "abort")?;
                    let error_cls = _interp.get_attr(self_val.0, "error")?;
                    if let PyErrorKind::Raised(exc) = err.kind() {
                        let exc_s = value_to_str(_interp, exc).unwrap_or_default();
                        if is_instance(_interp, exc, abort_cls).unwrap_or(false) {
                            return Err(raise_self_exc(
                                _interp,
                                self_val.0,
                                "abort",
                                Value::string(format!("command: {} => {exc_s}", name.0)),
                            ));
                        }
                        if is_instance(_interp, exc, error_cls).unwrap_or(false) {
                            return Err(raise_self_exc(
                                _interp,
                                self_val.0,
                                "error",
                                Value::string(format!("command: {} => {exc_s}", name.0)),
                            ));
                        }
                    }
                    return Err(err);
                }
            };
            if !logout {
                call_method(_interp, self_val.0, "_check_bye", &[])?;
            }
            let typ = py_getitem(_interp, &pair, Value::int(0))?;
            let data = py_getitem(_interp, &pair, Value::int(1))?;
            if typ.as_str() == Some("BAD") {
                let data_r = render_value_repr(_interp, &data)?;
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "error",
                    Value::string(format!("{} command error: BAD {data_r}", name.0)),
                ));
            }
            Ok(pair)
        }

        fn _get_capabilities(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pair = call_method(_interp, self_val.0, "capability", &[])?;
            let dat = py_getitem(_interp, &pair, Value::int(1))?;
            let items = _interp.collect_iterable(&dat)?;
            if items.len() == 1 && items[0].is_none() {
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "error",
                    Value::string("no CAPABILITY response from server"),
                ));
            }
            let last = items
                .last()
                .cloned()
                .unwrap_or_else(|| Value::bytes(Vec::new()));
            let enc = get_str_attr(_interp, self_val.0, "_encoding", "ascii")?;
            let decoded = match last.kind() {
                ValueKind::Bytes(_) => {
                    let s = call_method(_interp, &last, "decode", &[Value::string(enc)])?;
                    value_to_str(_interp, &s)?
                }
                _ => value_to_str(_interp, &last)?,
            };
            let caps: Vec<Value> = decoded
                .to_ascii_uppercase()
                .split_whitespace()
                .map(Value::string)
                .collect();
            _interp.assign_attr(self_val.0.clone(), "capabilities", Value::tuple(caps))?;
            Ok(Value::none())
        }

        fn _get_response(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let resp_val = call_method(_interp, self_val.0, "_get_line", &[])?;
            let resp = match resp_val.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => Vec::new(),
            };
            let tagpre_val = _interp.get_attr(self_val.0, "tagpre")?;
            let tagpre = match tagpre_val.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => Vec::new(),
            };
            let enc = get_str_attr(_interp, self_val.0, "_encoding", "ascii")?;

            let (typ_str, dat_bytes): (String, Vec<u8>) =
                if let Some((tag_b, typ_b, dat_b)) = match_tagged(&tagpre, &resp) {
                    let tc = _interp.get_attr(self_val.0, "tagged_commands")?;
                    let tag_key = PyKey::Bytes(Rc::new(tag_b.to_vec()));
                    let exists = tc.as_dict().is_some_and(|d| d.contains_key(&tag_key));
                    if !exists {
                        let r = render_value_repr(_interp, &resp_val)?;
                        return Err(raise_self_exc(
                            _interp,
                            self_val.0,
                            "abort",
                            Value::string(format!("unexpected tagged response: {r}")),
                        ));
                    }
                    let t_s = std::str::from_utf8(typ_b).unwrap_or("").to_string();
                    let entry = Value::tuple(vec![
                        Value::string(&t_s),
                        Value::list(vec![Value::bytes(dat_b.to_vec())]),
                    ]);
                    tc.dict_insert(tag_key, entry)?;
                    (t_s, dat_b.to_vec())
                } else if let Some((typ_b, dat_opt)) = match_untagged_response(&resp) {
                    let t_s = std::str::from_utf8(typ_b).unwrap_or("").to_string();
                    let mut dat_cur = dat_opt.map(|b| b.to_vec()).unwrap_or_default();
                    while let Some(size) = match_literal_size(&dat_cur) {
                        let lit_data = call_method(
                            _interp,
                            self_val.0,
                            "read",
                            &[Value::int(size as i64)],
                        )?;
                        let pair = Value::tuple(vec![Value::bytes(dat_cur), lit_data]);
                        call_method(
                            _interp,
                            self_val.0,
                            "_append_untagged",
                            &[Value::string(&t_s), pair],
                        )?;
                        let next_line = call_method(_interp, self_val.0, "_get_line", &[])?;
                        dat_cur = match next_line.kind() {
                            ValueKind::Bytes(b) => b.as_ref().clone(),
                            _ => Vec::new(),
                        };
                    }
                    call_method(
                        _interp,
                        self_val.0,
                        "_append_untagged",
                        &[Value::string(&t_s), Value::bytes(dat_cur.clone())],
                    )?;
                    (t_s, dat_cur)
                } else if let Some((num_b, typ_b, dat2_opt)) = match_untagged_status(&resp) {
                    let t_s = std::str::from_utf8(typ_b).unwrap_or("").to_string();
                    let mut dat_cur = num_b.to_vec();
                    if let Some(d2) = dat2_opt
                        && !d2.is_empty()
                    {
                        dat_cur.push(b' ');
                        dat_cur.extend_from_slice(d2);
                    }
                    while let Some(size) = match_literal_size(&dat_cur) {
                        let lit_data = call_method(
                            _interp,
                            self_val.0,
                            "read",
                            &[Value::int(size as i64)],
                        )?;
                        let pair = Value::tuple(vec![Value::bytes(dat_cur), lit_data]);
                        call_method(
                            _interp,
                            self_val.0,
                            "_append_untagged",
                            &[Value::string(&t_s), pair],
                        )?;
                        let next_line = call_method(_interp, self_val.0, "_get_line", &[])?;
                        dat_cur = match next_line.kind() {
                            ValueKind::Bytes(b) => b.as_ref().clone(),
                            _ => Vec::new(),
                        };
                    }
                    call_method(
                        _interp,
                        self_val.0,
                        "_append_untagged",
                        &[Value::string(&t_s), Value::bytes(dat_cur.clone())],
                    )?;
                    (t_s, dat_cur)
                } else if let Some(cont_opt) = match_continuation(&resp) {
                    let cont_val = match cont_opt {
                        Some(b) => Value::bytes(b.to_vec()),
                        None => Value::none(),
                    };
                    _interp.assign_attr(self_val.0.clone(), "continuation_response", cont_val)?;
                    return Ok(Value::none());
                } else {
                    let r = render_value_repr(_interp, &resp_val)?;
                    return Err(raise_self_exc(
                        _interp,
                        self_val.0,
                        "abort",
                        Value::string(format!("unexpected response: {r}")),
                    ));
                };

            let _ = enc;
            if matches!(typ_str.as_str(), "OK" | "NO" | "BAD")
                && let Some((code_t, code_d)) = match_response_code(&dat_bytes)
            {
                let ct_s = std::str::from_utf8(code_t).unwrap_or("").to_string();
                let cd_val = match code_d {
                    Some(b) => Value::bytes(b.to_vec()),
                    None => Value::none(),
                };
                call_method(
                    _interp,
                    self_val.0,
                    "_append_untagged",
                    &[Value::string(ct_s), cd_val],
                )?;
            }

            Ok(resp_val)
        }

        fn _get_tagged_response(
            #[positional_only] self_val: PyValueRef<'_>,
            tag: PyValue,
            #[default(false.into())] expect_bye: PyBool,
        ) -> Result<Value> {
            loop {
                let tc = _interp.get_attr(self_val.0, "tagged_commands")?;
                let result = py_getitem(_interp, &tc, tag.0.clone())?;
                if !result.is_none() {
                    let _ = call_method(_interp, &tc, "pop", &[tag.0]);
                    return Ok(result);
                }
                if expect_bye.0 {
                    let ur = _interp.get_attr(self_val.0, "untagged_responses")?;
                    let bye = call_method(
                        _interp,
                        &ur,
                        "pop",
                        &[Value::string("BYE"), Value::none()],
                    )?;
                    if !bye.is_none() {
                        return Ok(Value::tuple(vec![Value::string("BYE"), bye]));
                    }
                }
                call_method(_interp, self_val.0, "_check_bye", &[])?;
                call_method(_interp, self_val.0, "_get_response", &[])?;
            }
        }

        fn _get_line(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let line_val = call_method(_interp, self_val.0, "readline", &[])?;
            let line = match line_val.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => Vec::new(),
            };
            if line.is_empty() {
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "abort",
                    Value::string("socket error: EOF"),
                ));
            }
            if !line.ends_with(CRLF_BYTES) {
                let r = render_value_repr(_interp, &line_val)?;
                return Err(raise_self_exc(
                    _interp,
                    self_val.0,
                    "abort",
                    Value::string(format!("socket error: unterminated line: {r}")),
                ));
            }
            Ok(Value::bytes(line[..line.len() - 2].to_vec()))
        }

        fn _new_tag(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let tagpre_val = _interp.get_attr(self_val.0, "tagpre")?;
            let mut tag = match tagpre_val.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => Vec::new(),
            };
            let tagnum = get_int_attr(_interp, self_val.0, "tagnum", 0)?;
            tag.extend_from_slice(tagnum.to_string().as_bytes());
            _interp.assign_attr(self_val.0.clone(), "tagnum", Value::int(tagnum + 1))?;
            let tc = _interp.get_attr(self_val.0, "tagged_commands")?;
            tc.dict_insert(PyKey::Bytes(Rc::new(tag.clone())), Value::none())?;
            Ok(Value::bytes(tag))
        }

        fn _quote(#[positional_only] _self: PyValueRef<'_>, arg: PyStr<'_>) -> Result<Value> {
            let _ = _interp;
            let escaped = arg.0.replace('\\', "\\\\").replace('"', "\\\"");
            Ok(Value::string(format!("\"{escaped}\"")))
        }

        fn _simple_command(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let mut cmd_args = Vec::with_capacity(args.len() + 1);
            cmd_args.push(Value::string(&*name.0));
            cmd_args.extend_from_slice(args);
            let tag = call_method(_interp, self_val.0, "_command", &cmd_args)?;
            call_method(
                _interp,
                self_val.0,
                "_command_complete",
                &[Value::string(&*name.0), tag],
            )
        }

        fn _untagged_response(
            #[positional_only] self_val: PyValueRef<'_>,
            typ: PyValue,
            dat: PyValue,
            name: PyStr<'_>,
        ) -> Result<Value> {
            if typ.0.as_str() == Some("NO") {
                return Ok(Value::tuple(vec![typ.0, dat.0]));
            }
            let ur = _interp.get_attr(self_val.0, "untagged_responses")?;
            let exists = ur
                .as_dict()
                .is_some_and(|d| d.contains_key(&PyKey::str_from(&name.0)));
            if !exists {
                return Ok(Value::tuple(vec![typ.0, Value::list(vec![Value::none()])]));
            }
            let data = call_method(_interp, &ur, "pop", &[Value::string(&*name.0)])?;
            Ok(Value::tuple(vec![typ.0, data]))
        }
    }

    // ── IMAP4_SSL ────────────────────────────────────────────────────────────

    class IMAP4_SSL("IMAP4") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] host: PyStr<'_>,
            #[default(IMAP4_SSL_PORT_VAL.into())] port: PyIndex,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            ssl_context: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let ctx = match ssl_context.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let ssl_mod = _interp.load_module("ssl")?;
                    let mk = _interp.get_attr(&ssl_mod, "_create_stdlib_context")?;
                    call_pos(_interp, mk, &[])?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "ssl_context", ctx)?;
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let timeout_val = timeout.as_value().cloned().unwrap_or_else(Value::none);
            imap4_init_impl(_interp, self_val.0, &host.0, port_i64, timeout_val)?;
            Ok(Value::none())
        }

        fn _create_socket(
            #[positional_only] self_val: PyValueRef<'_>,
            timeout: PyValue,
        ) -> Result<Value> {
            let sock = imap4_create_socket_impl(_interp, self_val.0, timeout.0)?;
            let ctx = _interp.get_attr(self_val.0, "ssl_context")?;
            let host = _interp.get_attr(self_val.0, "host")?;
            call_method_kw(
                _interp,
                &ctx,
                "wrap_socket",
                &[sock],
                &[("server_hostname", host)],
            )
        }

        fn open(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] host: PyStr<'_>,
            #[default(IMAP4_SSL_PORT_VAL.into())] port: PyIndex,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let port_i64 = port.expect_i64(FN_NAME, "port")?;
            let timeout_val = timeout.as_value().cloned().unwrap_or_else(Value::none);
            imap4_open_impl(_interp, self_val.0, &host.0, port_i64, timeout_val)?;
            Ok(Value::none())
        }
    }

    // ── IMAP4_stream ─────────────────────────────────────────────────────────

    class IMAP4_stream("IMAP4") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            command: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "command", command.0)?;
            imap4_init_impl(_interp, self_val.0, "", IMAP4_PORT_VAL, Value::none())?;
            Ok(Value::none())
        }

        fn open(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _host: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _port: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "host", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "port", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "sock", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "file", Value::none())?;
            let cmd = _interp.get_attr(self_val.0, "command")?;
            let subp = _interp.load_module("subprocess")?;
            let popen = _interp.get_attr(&subp, "Popen")?;
            let pipe = _interp.get_attr(&subp, "PIPE")?;
            let io_mod = _interp.load_module("io")?;
            let def_buf = _interp.get_attr(&io_mod, "DEFAULT_BUFFER_SIZE")?;
            let proc = call_kw(
                _interp,
                popen,
                &[cmd],
                &[
                    ("bufsize", def_buf),
                    ("stdin", pipe.clone()),
                    ("stdout", pipe),
                    ("shell", Value::bool_(true)),
                    ("close_fds", Value::bool_(true)),
                ],
            )?;
            let stdin = _interp.get_attr(&proc, "stdin")?;
            let stdout = _interp.get_attr(&proc, "stdout")?;
            _interp.assign_attr(self_val.0.clone(), "process", proc)?;
            _interp.assign_attr(self_val.0.clone(), "writefile", stdin)?;
            _interp.assign_attr(self_val.0.clone(), "readfile", stdout)?;
            Ok(Value::none())
        }

        fn read(#[positional_only] self_val: PyValueRef<'_>, size: PyIndex) -> Result<Value> {
            let n = size.expect_i64(FN_NAME, "size")?;
            let rf = _interp.get_attr(self_val.0, "readfile")?;
            call_method(_interp, &rf, "read", &[Value::int(n)])
        }

        fn readline(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let rf = _interp.get_attr(self_val.0, "readfile")?;
            call_method(_interp, &rf, "readline", &[])
        }

        fn send(#[positional_only] self_val: PyValueRef<'_>, data: PyValue) -> Result<Value> {
            let wf = _interp.get_attr(self_val.0, "writefile")?;
            call_method(_interp, &wf, "write", &[data.0])?;
            call_method(_interp, &wf, "flush", &[])?;
            Ok(Value::none())
        }

        fn shutdown(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let rf = _interp.get_attr(self_val.0, "readfile")?;
            let wf = _interp.get_attr(self_val.0, "writefile")?;
            let proc = _interp.get_attr(self_val.0, "process")?;
            let _ = call_method(_interp, &rf, "close", &[]);
            let _ = call_method(_interp, &wf, "close", &[]);
            call_method(_interp, &proc, "wait", &[])?;
            Ok(Value::none())
        }
    }
}
