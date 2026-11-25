// `http.cookiejar` module — 100% native-Rust Python 3.12 implementation using
// PyRust's expanded macro toolkit (`pyrust_module!`, `exception`, `class`,
// `#[varkwargs]`, `PyPath`).
//
// Provides `LoadError`, `Cookie`, `CookiePolicy`, `DefaultCookiePolicy`,
// `CookieJar`, `FileCookieJar`, `MozillaCookieJar`, `LWPCookieJar`, and
// standard RFC 2109/2965/6265 date, domain, and header helpers.
//
// Reference: <https://docs.python.org/3/library/http.cookiejar.html>

use std::time::{SystemTime, UNIX_EPOCH};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyInt, PyPath, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr, value_class};
use crate::value::{PyDict, PyKey, Value, ValueKind};

// ── Pure-Rust calendar & HTTP/ISO date helpers ───────────────────────────────

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn now_unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Howard Hinnant's civil-from-days algorithm: days since 1970-01-01 -> (Y, M, D).
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

/// Howard Hinnant's days-from-civil algorithm: (Y, M, D) -> days since 1970-01-01.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m_u = m as u64;
    let doy = (153 * (if m_u > 2 { m_u - 3 } else { m_u + 9 }) + 2) / 5 + (d as u64) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + (doe as i64) - 719_468
}

/// Weekday index matching Python (`0 = Mon .. 6 = Sun`) from days since 1970-01-01 (Thu = 3).
fn weekday_from_days(days: i64) -> usize {
    (days + 3).rem_euclid(7) as usize
}

fn unix_to_ymdhms(ts: i64) -> (i64, u32, u32, u32, u32, u32, usize) {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400) as u32;
    let (y, m, d) = civil_from_days(days);
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    let ss = secs % 60;
    let wd = weekday_from_days(days);
    (y, m, d, hh, mm, ss, wd)
}

fn ymdhms_to_unix(y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let days = days_from_civil(y, m, d);
    Some(days * 86_400 + (hh as i64) * 3600 + (mm as i64) * 60 + (ss as i64))
}

fn month_name_to_num(s: &str) -> Option<u32> {
    let prefix: String = s.chars().take(3).collect();
    match prefix.to_ascii_lowercase().as_str() {
        "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "aug" => Some(8),
        "sep" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dec" => Some(12),
        _ => None,
    }
}

fn parse_hms(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split(':');
    let hh: u32 = parts.next()?.parse().ok()?;
    let mm: u32 = parts.next()?.parse().ok()?;
    let ss: u32 = parts.next().unwrap_or("0").parse().ok()?;
    Some((hh, mm, ss))
}

fn expand_year(y: i64) -> i64 {
    if (0..100).contains(&y) {
        if y < 70 { 2000 + y } else { 1900 + y }
    } else {
        y
    }
}

fn parse_tz_offset(tz: &str) -> i64 {
    let t = tz.trim().to_ascii_uppercase();
    if matches!(t.as_str(), "" | "GMT" | "UTC" | "UT" | "Z") {
        return 0;
    }
    if (t.starts_with('+') || t.starts_with('-')) && t.len() >= 5 {
        let sign = if t.starts_with('-') { -1 } else { 1 };
        let digits: String = t[1..].chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 4
            && let (Ok(hh), Ok(mm)) = (digits[0..2].parse::<i64>(), digits[2..4].parse::<i64>())
        {
            return sign * (hh * 3600 + mm * 60);
        }
    }
    0
}

fn parse_iso2time(text: &str) -> Option<i64> {
    let s = text.trim();
    if s.len() < 10 {
        return None;
    }
    let date_part = &s[..10];
    let mut dp = date_part.split('-');
    let y: i64 = dp.next()?.parse().ok()?;
    let m: u32 = dp.next()?.parse().ok()?;
    let d: u32 = dp.next()?.parse().ok()?;
    let rest = s[10..].trim();
    if rest.is_empty() {
        return ymdhms_to_unix(y, m, d, 0, 0, 0);
    }
    let rest = rest
        .strip_prefix('T')
        .or_else(|| rest.strip_prefix('t'))
        .unwrap_or(rest)
        .trim();
    let (time_str, tz_str) = if let Some(stripped) = rest.strip_suffix('Z') {
        (stripped, "Z")
    } else if let Some(stripped) = rest.strip_suffix('z') {
        (stripped, "Z")
    } else if let Some(idx) = rest.find(['+', '-']) {
        (&rest[..idx], &rest[idx..])
    } else if let Some((t, tz)) = rest.split_once(' ') {
        (t, tz)
    } else {
        (rest, "")
    };
    let (hh, mm, ss) = parse_hms(time_str.trim())?;
    let base = ymdhms_to_unix(y, m, d, hh, mm, ss)?;
    Some(base - parse_tz_offset(tz_str))
}

fn parse_http2time(text: &str) -> Option<i64> {
    let s = text.trim();
    if s.is_empty() {
        return None;
    }
    // Strip optional weekday prefix ("Sun, " or "Sunday, ").
    let body = if let Some((_, after)) = s.split_once(',') {
        after.trim()
    } else {
        s
    };
    // Try ISO-8601 first if it starts with YYYY-MM-DD.
    if body.len() >= 10
        && body.as_bytes()[4] == b'-'
        && body.as_bytes()[7] == b'-'
        && let Some(ts) = parse_iso2time(body)
    {
        return Some(ts);
    }

    let tokens: Vec<&str> = body.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    // Format 1: RFC 850 / Netscape "DD-Mon-YYYY HH:MM:SS GMT" or "DD-Mon-YY HH:MM:SS GMT"
    if tokens[0].contains('-') && tokens.len() >= 2 {
        let dparts: Vec<&str> = tokens[0].split('-').collect();
        if dparts.len() == 3
            && let (Ok(d), Some(m), Ok(y_raw)) = (
                dparts[0].parse::<u32>(),
                month_name_to_num(dparts[1]),
                dparts[2].parse::<i64>(),
            )
            && let Some((hh, mm, ss)) = parse_hms(tokens[1])
        {
            let tz = tokens.get(2).copied().unwrap_or("");
            let base = ymdhms_to_unix(expand_year(y_raw), m, d, hh, mm, ss)?;
            return Some(base - parse_tz_offset(tz));
        }
    }

    // Format 2: RFC 1123 / IMF-fixdate "DD Mon YYYY HH:MM:SS GMT"
    if tokens.len() >= 4
        && let (Ok(d), Some(m), Ok(y_raw)) = (
            tokens[0].parse::<u32>(),
            month_name_to_num(tokens[1]),
            tokens[2].parse::<i64>(),
        )
        && let Some((hh, mm, ss)) = parse_hms(tokens[3])
    {
        let tz = tokens.get(4).copied().unwrap_or("");
        let base = ymdhms_to_unix(expand_year(y_raw), m, d, hh, mm, ss)?;
        return Some(base - parse_tz_offset(tz));
    }

    // Format 3: asctime "[Wdy] Mon DD HH:MM:SS YYYY"
    let start = if tokens.len() >= 5 && month_name_to_num(tokens[1]).is_some() {
        1
    } else {
        0
    };
    if tokens.len() >= start + 4
        && let (Some(m), Ok(d), Some((hh, mm, ss)), Ok(y_raw)) = (
            month_name_to_num(tokens[start]),
            tokens[start + 1].parse::<u32>(),
            parse_hms(tokens[start + 2]),
            tokens[start + 3].parse::<i64>(),
        )
    {
        let tz = tokens.get(start + 4).copied().unwrap_or("");
        let base = ymdhms_to_unix(expand_year(y_raw), m, d, hh, mm, ss)?;
        return Some(base - parse_tz_offset(tz));
    }

    None
}

fn format_time2isoz(ts: Option<f64>) -> String {
    let t = ts.unwrap_or_else(now_unix_secs).floor() as i64;
    let (y, m, d, hh, mm, ss, _) = unix_to_ymdhms(t);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}Z")
}

fn format_time2netscape(ts: Option<f64>) -> String {
    let t = ts.unwrap_or_else(now_unix_secs).floor() as i64;
    let (y, m, d, hh, mm, ss, wd) = unix_to_ymdhms(t);
    let wdy = WEEKDAYS[wd];
    let mon = MONTHS[(m - 1) as usize];
    format!("{wdy}, {d:02}-{mon}-{y:04} {hh:02}:{mm:02}:{ss:02} GMT")
}

// ── Domain, host, path, and header parsing helpers ───────────────────────────

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

fn cj_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("http.cookiejar")?;
    interp.get_attr(&m, name)
}

fn raise_load_error(interp: &mut Interpreter, msg: impl Into<String>) -> PyError {
    let msg = msg.into();
    match cj_attr(interp, "LoadError").and_then(|cls| call_pos(interp, cls, &[Value::string(&msg)]))
    {
        Ok(exc) => PyError::raised(exc),
        Err(e) => e,
    }
}

fn is_ipv4_literal(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.parse::<u8>().is_ok())
}

fn is_hdn_str(text: &str) -> bool {
    if text.is_empty() || is_ipv4_literal(text) {
        return false;
    }
    if text.starts_with('[') && text.ends_with(']') {
        return false;
    }
    let parts: Vec<&str> = text.split('.').collect();
    for part in &parts {
        if part.is_empty() {
            return false;
        }
        if !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return false;
        }
    }
    true
}

fn domain_match_str(a: &str, b: &str) -> bool {
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    if a == b {
        return true;
    }
    if !is_hdn_str(&a) {
        return false;
    }
    if let Some(i) = a.rfind(&b)
        && i + b.len() == a.len()
        && b.starts_with('.')
        && !a.starts_with('.')
    {
        return true;
    }
    false
}

fn user_domain_match_str(a: &str, b: &str) -> bool {
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    if !(a.starts_with('.') && b.starts_with('.')) {
        return a == b;
    }
    if a == b {
        return true;
    }
    if !is_hdn_str(&a[1..]) {
        return false;
    }
    a.ends_with(&b)
}

fn reach_str(h: &str) -> String {
    if let Some(i) = h.find('.') {
        let rest = &h[i..];
        if !is_hdn_str(h) {
            return h.to_string();
        }
        if rest.matches('.').count() >= 2 || rest == ".local" {
            return rest.to_string();
        }
    }
    h.to_string()
}

fn strip_port(host: &str) -> (&str, Option<&str>) {
    if let Some(rest) = host.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let addr = &host[..=end + 1];
            let tail = &rest[end + 1..];
            if let Some(p) = tail.strip_prefix(':') {
                return (addr, if p.is_empty() { None } else { Some(p) });
            }
            return (addr, None);
        }
        return (host, None);
    }
    if let Some((h, p)) = host.rsplit_once(':')
        && p.bytes().all(|b| b.is_ascii_digit())
    {
        return (h, if p.is_empty() { None } else { Some(p) });
    }
    (host, None)
}

fn url_scheme_host_path(url: &str) -> (String, String, String) {
    let (scheme, rest) = if let Some((sc, r)) = url.split_once("://") {
        (sc.to_ascii_lowercase(), r)
    } else if let Some((sc, r)) = url.split_once(':') {
        (sc.to_ascii_lowercase(), r)
    } else {
        (String::new(), url)
    };
    let rest = rest.split('#').next().unwrap_or(rest);
    let (netloc, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => match rest.find('?') {
            Some(idx) => (&rest[..idx], &rest[idx..]),
            None => (rest, ""),
        },
    };
    let host_port = netloc.rsplit_once('@').map(|(_, h)| h).unwrap_or(netloc);
    let path = path_and_query.split('?').next().unwrap_or(path_and_query);
    (scheme, host_port.to_string(), path.to_string())
}

fn get_request_full_url(interp: &mut Interpreter, request: &Value) -> Result<String> {
    if let Ok(m) = interp.get_attr(request, "get_full_url") {
        let v = call_pos(interp, m, &[])?;
        if let Some(s) = v.as_str() {
            return Ok(s.to_string());
        }
    }
    if let Ok(v) = interp.get_attr(request, "full_url")
        && let Some(s) = v.as_str()
    {
        return Ok(s.to_string());
    }
    Ok(String::new())
}

fn request_host_str(interp: &mut Interpreter, request: &Value) -> Result<String> {
    let url = get_request_full_url(interp, request)?;
    let (_, mut host_port, _) = url_scheme_host_path(&url);
    if host_port.is_empty()
        && let Ok(gh) = interp.get_attr(request, "get_header")
        && let Ok(v) = call_pos(interp, gh, &[Value::string("Host"), Value::string("")])
        && let Some(s) = v.as_str()
    {
        host_port = s.to_string();
    }
    let (host, _) = strip_port(&host_port);
    Ok(host.to_ascii_lowercase())
}

fn request_port_str(interp: &mut Interpreter, request: &Value) -> Result<String> {
    let url = get_request_full_url(interp, request)?;
    let (scheme, host_port, _) = url_scheme_host_path(&url);
    let (_, port_opt) = strip_port(&host_port);
    if let Some(p) = port_opt {
        return Ok(p.to_string());
    }
    if scheme == "https" {
        Ok("443".to_string())
    } else {
        Ok("80".to_string())
    }
}

fn eff_request_host_pair(interp: &mut Interpreter, request: &Value) -> Result<(String, String)> {
    let erhn = request_host_str(interp, request)?;
    let req_type = interp
        .get_attr(request, "type")
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let req_host = if !erhn.contains('.') && !is_ipv4_literal(&erhn) && !erhn.is_empty() {
        format!("{erhn}.local")
    } else {
        erhn.clone()
    };
    let erhn_eff = if erhn.is_empty() && req_type == "file" {
        "localhost.local".to_string()
    } else {
        req_host.clone()
    };
    Ok((erhn_eff, erhn))
}

fn request_path_str(interp: &mut Interpreter, request: &Value) -> Result<String> {
    let url = get_request_full_url(interp, request)?;
    let (_, _, path) = url_scheme_host_path(&url);
    if path.is_empty() || !path.starts_with('/') {
        return Ok(format!("/{path}"));
    }
    Ok(path)
}

fn default_cookie_path(req_path: &str) -> String {
    if let Some(idx) = req_path.rfind('/') {
        if idx == 0 {
            "/".to_string()
        } else {
            req_path[..idx].to_string()
        }
    } else {
        "/".to_string()
    }
}

fn request_scheme_str(interp: &mut Interpreter, request: &Value) -> Result<String> {
    if let Ok(t) = interp.get_attr(request, "type")
        && let Some(s) = t.as_str()
    {
        return Ok(s.to_ascii_lowercase());
    }
    let url = get_request_full_url(interp, request)?;
    Ok(url_scheme_host_path(&url).0)
}

fn request_origin_host_str(interp: &mut Interpreter, request: &Value) -> Result<String> {
    if let Ok(v) = interp.get_attr(request, "origin_req_host")
        && let Some(s) = v.as_str()
    {
        return Ok(s.to_ascii_lowercase());
    }
    request_host_str(interp, request)
}

fn request_unverifiable_bool(interp: &mut Interpreter, request: &Value) -> Result<bool> {
    if let Ok(v) = interp.get_attr(request, "unverifiable") {
        return interp.truthy_value(&v);
    }
    Ok(false)
}

/// Split a single `Set-Cookie` / `Set-Cookie2` / `Set-Cookie3` header into
/// `(key, Option<value>)` pairs (preserving quoted values and escaping).
fn parse_cookie_attrs_str(header: &str) -> Vec<(String, Option<String>)> {
    let mut pairs = Vec::new();
    let bytes = header.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Skip whitespace and stray semicolons.
        while i < len && (bytes[i].is_ascii_whitespace() || bytes[i] == b';') {
            i += 1;
        }
        if i >= len {
            break;
        }
        let start = i;
        while i < len && bytes[i] != b'=' && bytes[i] != b';' {
            i += 1;
        }
        let key = header[start..i].trim().to_string();
        if i < len && bytes[i] == b'=' {
            i += 1;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let val = if i < len && bytes[i] == b'"' {
                i += 1;
                let mut buf = String::new();
                while i < len {
                    if bytes[i] == b'\\' && i + 1 < len {
                        buf.push(bytes[i + 1] as char);
                        i += 2;
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        let ch = header[i..].chars().next().unwrap();
                        buf.push(ch);
                        i += ch.len_utf8();
                    }
                }
                while i < len && bytes[i] != b';' {
                    i += 1;
                }
                buf
            } else {
                let vstart = i;
                while i < len && bytes[i] != b';' {
                    i += 1;
                }
                header[vstart..i].trim().to_string()
            };
            pairs.push((key, Some(val)));
        } else if !key.is_empty() {
            pairs.push((key, None));
        }
    }
    pairs
}

fn escape_lwp_value(val: &str) -> String {
    let needs_quote = val.is_empty()
        || val
            .bytes()
            .any(|b| b <= 0x20 || matches!(b, b';' | b',' | b'"' | b'\\' | b'='));
    if !needs_quote {
        return val.to_string();
    }
    let mut out = String::with_capacity(val.len() + 2);
    out.push('"');
    for ch in val.chars() {
        if ch == '\\' || ch == '"' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

fn format_lwp_cookie_str(interp: &mut Interpreter, cookie: &Value) -> Result<String> {
    let name = interp
        .get_attr(cookie, "name")?
        .as_str()
        .unwrap_or_default()
        .to_string();
    let value_val = interp.get_attr(cookie, "value")?;
    let path = interp
        .get_attr(cookie, "path")?
        .as_str()
        .unwrap_or("/")
        .to_string();
    let domain = interp
        .get_attr(cookie, "domain")?
        .as_str()
        .unwrap_or_default()
        .to_string();
    let port_val = interp.get_attr(cookie, "port")?;
    let port_spec = get_attr_truthy(interp, cookie, "port_specified")?;
    let path_spec = get_attr_truthy(interp, cookie, "path_specified")?;
    let domain_dot = get_attr_truthy(interp, cookie, "domain_initial_dot")?;
    let secure = get_attr_truthy(interp, cookie, "secure")?;
    let expires_val = interp.get_attr(cookie, "expires")?;
    let discard = get_attr_truthy(interp, cookie, "discard")?;
    let comment_val = interp.get_attr(cookie, "comment")?;
    let comment_url_val = interp.get_attr(cookie, "comment_url")?;
    let version = interp.get_attr(cookie, "version")?.as_int().unwrap_or(0);

    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = value_val.as_str() {
        parts.push(format!("{name}={}", escape_lwp_value(v)));
    } else {
        parts.push(name);
    }
    parts.push(format!("path={}", escape_lwp_value(&path)));
    parts.push(format!("domain={}", escape_lwp_value(&domain)));
    if let Some(p) = port_val.as_str() {
        parts.push(format!("port={}", escape_lwp_value(p)));
    }
    if path_spec {
        parts.push("path_spec".to_string());
    }
    if port_spec {
        parts.push("port_spec".to_string());
    }
    if domain_dot {
        parts.push("domain_dot".to_string());
    }
    if secure {
        parts.push("secure".to_string());
    }
    if !expires_val.is_none()
        && let Some(t) = value_as_f64(&expires_val)
    {
        parts.push(format!("expires=\"{}\"", format_time2isoz(Some(t))));
    }
    if discard {
        parts.push("discard".to_string());
    }
    if let Some(c) = comment_val.as_str() {
        parts.push(format!("comment={}", escape_lwp_value(c)));
    }
    if let Some(cu) = comment_url_val.as_str() {
        parts.push(format!("commenturl={}", escape_lwp_value(cu)));
    }
    if let Ok(rest) = interp.get_attr(cookie, "_rest")
        && let Some(d) = rest.as_dict()
    {
        let mut keys: Vec<(String, Value)> = Vec::new();
        for (k, v) in d.iter() {
            if let PyKey::Str(sv) = k
                && let Some(ks) = sv.as_str()
            {
                keys.push((ks.to_string(), v.clone()));
            }
        }
        keys.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in keys {
            if v.is_none() {
                parts.push(k);
            } else if let Some(vs) = v.as_str() {
                parts.push(format!("{k}={}", escape_lwp_value(vs)));
            }
        }
    }
    parts.push(format!("version={version}"));
    Ok(parts.join("; "))
}

/// Extract all `Set-Cookie` and `Set-Cookie2` header values from `response`.
fn extract_set_cookie_headers(
    interp: &mut Interpreter,
    response: &Value,
) -> Result<(Vec<String>, Vec<String>)> {
    let headers_obj = if let Ok(info_fn) = interp.get_attr(response, "info") {
        call_pos(interp, info_fn, &[])?
    } else if let Ok(h) = interp.get_attr(response, "headers") {
        h
    } else if let Ok(m) = interp.get_attr(response, "msg") {
        m
    } else {
        return Ok((Vec::new(), Vec::new()));
    };

    let fetch_all = |interp: &mut Interpreter, hdr_name: &str| -> Result<Vec<String>> {
        if let Ok(get_all) = interp.get_attr(&headers_obj, "get_all") {
            let res = call_pos(interp, get_all, &[Value::string(hdr_name)])?;
            if !res.is_none() {
                let items = interp.collect_iterable(&res)?;
                return Ok(items
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect());
            }
            return Ok(Vec::new());
        }
        if let Ok(get_fn) = interp.get_attr(&headers_obj, "get") {
            let res = call_pos(interp, get_fn, &[Value::string(hdr_name)])?;
            if let Some(s) = res.as_str() {
                return Ok(vec![s.to_string()]);
            }
        }
        Ok(Vec::new())
    };

    let ns = fetch_all(interp, "Set-Cookie")?;
    let rfc = fetch_all(interp, "Set-Cookie2")?;
    Ok((ns, rfc))
}

#[allow(clippy::too_many_arguments)]
fn construct_cookie(
    interp: &mut Interpreter,
    version: i64,
    name: String,
    value: Value,
    port: Value,
    port_specified: bool,
    domain: String,
    domain_specified: bool,
    domain_initial_dot: bool,
    path: String,
    path_specified: bool,
    secure: bool,
    expires: Value,
    discard: bool,
    comment: Value,
    comment_url: Value,
    rest: Value,
    rfc2109: bool,
) -> Result<Value> {
    let cookie_cls = cj_attr(interp, "Cookie")?;
    call_pos(
        interp,
        cookie_cls,
        &[
            Value::int(version),
            Value::string(name),
            value,
            port,
            Value::bool_(port_specified),
            Value::string(domain),
            Value::bool_(domain_specified),
            Value::bool_(domain_initial_dot),
            Value::string(path),
            Value::bool_(path_specified),
            Value::bool_(secure),
            expires,
            Value::bool_(discard),
            comment,
            comment_url,
            rest,
            Value::bool_(rfc2109),
        ],
    )
}

fn cookie_from_set_cookie_header(
    interp: &mut Interpreter,
    header: &str,
    request: &Value,
    is_rfc2965: bool,
) -> Result<Option<Value>> {
    let attrs = parse_cookie_attrs_str(header);
    if attrs.is_empty() {
        return Ok(None);
    }
    let (name, raw_val) = &attrs[0];
    if name.is_empty() {
        return Ok(None);
    }
    let value = match raw_val {
        Some(v) => Value::string(v.clone()),
        None => Value::none(),
    };

    let (eff_host, _) = eff_request_host_pair(interp, request)?;
    let req_path = request_path_str(interp, request)?;

    let mut version: i64 = if is_rfc2965 { 1 } else { 0 };
    let mut port = Value::none();
    let mut port_specified = false;
    let mut domain = eff_host;
    let mut domain_specified = false;
    let mut domain_initial_dot = false;
    let mut path = default_cookie_path(&req_path);
    let mut path_specified = false;
    let mut secure = false;
    let mut expires = Value::none();
    let mut has_max_age = false;
    let mut discard = true;
    let mut comment = Value::none();
    let mut comment_url = Value::none();
    let mut rfc2109 = false;
    let mut rest_dict = PyDict::default();

    for (k, v_opt) in attrs.iter().skip(1) {
        let kl = k.to_ascii_lowercase();
        match kl.as_str() {
            "domain" => {
                if let Some(d) = v_opt
                    && !d.is_empty()
                {
                    let d_low = d.to_ascii_lowercase();
                    domain_initial_dot = d_low.starts_with('.');
                    domain = if domain_initial_dot {
                        d_low
                    } else {
                        format!(".{d_low}")
                    };
                    domain_specified = true;
                }
            }
            "path" => {
                if let Some(p) = v_opt
                    && !p.is_empty()
                {
                    path = p.clone();
                    path_specified = true;
                }
            }
            "secure" => {
                secure = true;
            }
            "expires" => {
                if !has_max_age
                    && let Some(e_str) = v_opt
                    && let Some(ts) = parse_http2time(e_str)
                {
                    expires = Value::int(ts);
                    discard = false;
                }
            }
            "max-age" => {
                if let Some(ma_str) = v_opt
                    && let Ok(delta) = ma_str.trim().parse::<i64>()
                {
                    let ts = (now_unix_secs().floor() as i64) + delta;
                    expires = Value::int(ts);
                    discard = false;
                    has_max_age = true;
                }
            }
            "version" => {
                if let Some(ver_str) = v_opt
                    && let Ok(ver) = ver_str.trim().parse::<i64>()
                {
                    version = ver;
                    if !is_rfc2965 {
                        rfc2109 = true;
                    }
                }
            }
            "port" => {
                port_specified = true;
                if let Some(p_str) = v_opt {
                    port = Value::string(p_str.clone());
                } else {
                    port = Value::string(request_port_str(interp, request)?);
                }
            }
            "comment" => {
                if let Some(c) = v_opt {
                    comment = Value::string(c.clone());
                }
            }
            "commenturl" => {
                if let Some(cu) = v_opt {
                    comment_url = Value::string(cu.clone());
                }
            }
            "discard" => {
                discard = true;
            }
            _ => {
                let rv = match v_opt {
                    Some(v) => Value::string(v.clone()),
                    None => Value::none(),
                };
                rest_dict.insert(PyKey::str_from(k), rv);
            }
        }
    }

    let cookie = construct_cookie(
        interp,
        version,
        name.clone(),
        value,
        port,
        port_specified,
        domain,
        domain_specified,
        domain_initial_dot,
        path,
        path_specified,
        secure,
        expires,
        discard,
        comment,
        comment_url,
        Value::dict(rest_dict),
        rfc2109,
    )?;
    Ok(Some(cookie))
}

fn collect_all_cookies_sorted(interp: &mut Interpreter, jar: &Value) -> Result<Vec<Value>> {
    let cookies_dict = interp.get_attr(jar, "_cookies")?;
    let Some(dom_map) = cookies_dict.as_dict() else {
        return Ok(Vec::new());
    };
    let mut domains: Vec<(String, Value)> = Vec::new();
    for (k, v) in dom_map.iter() {
        if let PyKey::Str(sv) = k
            && let Some(ds) = sv.as_str()
        {
            domains.push((ds.to_string(), v.clone()));
        }
    }
    domains.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = Vec::new();
    for (_, path_dict_val) in domains {
        let Some(path_map) = path_dict_val.as_dict() else {
            continue;
        };
        let mut paths: Vec<(String, Value)> = Vec::new();
        for (pk, pv) in path_map.iter() {
            if let PyKey::Str(sv) = pk
                && let Some(ps) = sv.as_str()
            {
                paths.push((ps.to_string(), pv.clone()));
            }
        }
        paths.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, name_dict_val) in paths {
            let Some(name_map) = name_dict_val.as_dict() else {
                continue;
            };
            let mut names: Vec<(String, Value)> = Vec::new();
            for (nk, nv) in name_map.iter() {
                if let PyKey::Str(sv) = nk
                    && let Some(ns) = sv.as_str()
                {
                    names.push((ns.to_string(), nv.clone()));
                }
            }
            names.sort_by(|a, b| a.0.cmp(&b.0));
            for (_, cookie) in names {
                out.push(cookie);
            }
        }
    }
    Ok(out)
}

fn snapshot_cookies_dict(cookies_val: &Value) -> Value {
    let mut top = PyDict::default();
    if let Some(dom_map) = cookies_val.as_dict() {
        for (dk, dv) in dom_map.iter() {
            let mut pdict = PyDict::default();
            if let Some(pm) = dv.as_dict() {
                for (pk, pv) in pm.iter() {
                    let mut ndict = PyDict::default();
                    if let Some(nm) = pv.as_dict() {
                        for (nk, nv) in nm.iter() {
                            ndict.insert(nk.clone(), nv.clone());
                        }
                    }
                    pdict.insert(pk.clone(), Value::dict(ndict));
                }
            }
            top.insert(dk.clone(), Value::dict(pdict));
        }
    }
    Value::dict(top)
}

fn resolve_jar_filename(
    interp: &mut Interpreter,
    jar: &Value,
    filename: OptValueRef<'_>,
) -> Result<String> {
    let chosen = match filename.as_value() {
        Some(v) if !v.is_none() => v.clone(),
        _ => interp.get_attr(jar, "filename")?,
    };
    if chosen.is_none() {
        return Err(PyError::named("ValueError", "missing filename".to_string()));
    }
    if let Some(s) = chosen.as_str() {
        return Ok(s.to_string());
    }
    if let Ok(fspath_fn) = interp.get_attr(&chosen, "__fspath__") {
        let p = call_pos(interp, fspath_fn, &[])?;
        if let Some(s) = p.as_str() {
            return Ok(s.to_string());
        }
    }
    Err(PyError::named(
        "TypeError",
        "filename must be a string or PathLike object",
    ))
}

fn should_skip_cookie(
    interp: &mut Interpreter,
    cookie: &Value,
    ignore_discard: bool,
    ignore_expires: bool,
    now: f64,
) -> Result<bool> {
    if !ignore_discard && get_attr_truthy(interp, cookie, "discard")? {
        return Ok(true);
    }
    if !ignore_expires {
        let expired = call_method(interp, cookie, "is_expired", &[Value::float(now)])?;
        if interp.truthy_value(&expired)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_all_text_from_file(interp: &mut Interpreter, f: &Value) -> Result<String> {
    let data = call_method(interp, f, "read", &[])?;
    if let Some(s) = data.as_str() {
        return Ok(s.to_string());
    }
    if let ValueKind::Bytes(b) = data.kind() {
        return Ok(String::from_utf8_lossy(b).into_owned());
    }
    Err(PyError::named(
        "TypeError",
        "file read() did not return str",
    ))
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("Cookie"),
            Value::string("CookieJar"),
            Value::string("CookiePolicy"),
            Value::string("DefaultCookiePolicy"),
            Value::string("FileCookieJar"),
            Value::string("LWPCookieJar"),
            Value::string("LoadError"),
            Value::string("MozillaCookieJar"),
        ]),
        "DEFAULT_HTTP_PORT" => Value::string("80"),
    }

    /// Raised when `FileCookieJar.load()` fails to load a cookie file.
    exception LoadError(OSError);

    fn http2time(text: PyStr<'_>) -> Result<Value> {
        match parse_http2time(&text.0) {
            Some(ts) => Ok(Value::int(ts)),
            None => Ok(Value::none()),
        }
    }

    fn iso2time(text: PyStr<'_>) -> Result<Value> {
        match parse_iso2time(&text.0) {
            Some(ts) => Ok(Value::int(ts)),
            None => Ok(Value::none()),
        }
    }

    fn time2isoz(#[default(OptionalArg::Omitted)] t: OptValueRef<'_>) -> Result<Value> {
        let ts = t.as_value().filter(|v| !v.is_none()).and_then(value_as_f64);
        Ok(Value::string(format_time2isoz(ts)))
    }

    fn time2netscape(#[default(OptionalArg::Omitted)] t: OptValueRef<'_>) -> Result<Value> {
        let ts = t.as_value().filter(|v| !v.is_none()).and_then(value_as_f64);
        Ok(Value::string(format_time2netscape(ts)))
    }

    #[py_name = "is_HDN"]
    fn is_hdn(text: PyStr<'_>) -> Result<Value> {
        Ok(Value::bool_(is_hdn_str(&text.0)))
    }

    fn domain_match(a: PyStr<'_>, b: PyStr<'_>) -> Result<Value> {
        Ok(Value::bool_(domain_match_str(&a.0, &b.0)))
    }

    fn user_domain_match(a: PyStr<'_>, b: PyStr<'_>) -> Result<Value> {
        Ok(Value::bool_(user_domain_match_str(&a.0, &b.0)))
    }

    fn reach(h: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(reach_str(&h.0)))
    }

    fn request_host(request: PyValue) -> Result<Value> {
        Ok(Value::string(request_host_str(_interp, &request.0)?))
    }

    fn request_port(request: PyValue) -> Result<Value> {
        Ok(Value::string(request_port_str(_interp, &request.0)?))
    }

    fn request_path(request: PyValue) -> Result<Value> {
        Ok(Value::string(request_path_str(_interp, &request.0)?))
    }

    fn eff_request_host(request: PyValue) -> Result<Value> {
        let (eff, orig) = eff_request_host_pair(_interp, &request.0)?;
        Ok(Value::tuple(vec![Value::string(eff), Value::string(orig)]))
    }

    fn lwp_cookie_str(cookie: PyValue) -> Result<Value> {
        Ok(Value::string(format_lwp_cookie_str(_interp, &cookie.0)?))
    }

    // ── Cookie ───────────────────────────────────────────────────────────────

    class Cookie {
        #[allow(clippy::too_many_arguments)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            version: PyValue,
            name: PyValue,
            value: PyValue,
            port: PyValue,
            port_specified: PyValue,
            domain: PyValue,
            domain_specified: PyValue,
            domain_initial_dot: PyValue,
            path: PyValue,
            path_specified: PyValue,
            secure: PyValue,
            expires: PyValue,
            discard: PyValue,
            comment: PyValue,
            comment_url: PyValue,
            rest: PyValue,
            #[default(false.into())] rfc2109: PyBool,
        ) -> Result<Value> {
            let version_val = if version.0.is_none() {
                Value::int(0)
            } else {
                version.0
            };
            let port_spec = _interp.truthy_value(&port_specified.0)?;
            if port.0.is_none() && port_spec {
                return Err(PyError::named(
                    "ValueError",
                    "if port is None, port_specified must be false",
                ));
            }
            let mut rest_copy = PyDict::default();
            if let Some(d) = rest.0.as_dict() {
                for (k, v) in d.iter() {
                    rest_copy.insert(k.clone(), v.clone());
                }
            }
            let expires_norm = if expires.0.is_none() || expires.0.as_str() == Some("") {
                Value::none()
            } else if let Some(s) = expires.0.as_str() {
                match s.parse::<i64>() {
                    Ok(i) => Value::int(i),
                    Err(_) => expires.0,
                }
            } else {
                expires.0
            };
            let attrs: [(&str, Value); 17] = [
                ("version", version_val),
                ("name", name.0),
                ("value", value.0),
                ("port", port.0),
                ("port_specified", Value::bool_(port_spec)),
                ("domain", domain.0),
                (
                    "domain_specified",
                    Value::bool_(_interp.truthy_value(&domain_specified.0)?),
                ),
                (
                    "domain_initial_dot",
                    Value::bool_(_interp.truthy_value(&domain_initial_dot.0)?),
                ),
                ("path", path.0),
                (
                    "path_specified",
                    Value::bool_(_interp.truthy_value(&path_specified.0)?),
                ),
                ("secure", Value::bool_(_interp.truthy_value(&secure.0)?)),
                ("expires", expires_norm),
                ("discard", Value::bool_(_interp.truthy_value(&discard.0)?)),
                ("comment", comment.0),
                ("comment_url", comment_url.0),
                ("_rest", Value::dict(rest_copy)),
                ("rfc2109", Value::bool_(rfc2109.0)),
            ];
            for (k, v) in attrs {
                _interp.assign_attr(self_val.0.clone(), k, v)?;
            }
            Ok(Value::none())
        }

        fn has_nonstandard_attr(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let rest = _interp.get_attr(self_val.0, "_rest")?;
            let found = rest
                .as_dict()
                .is_some_and(|d| d.get(&PyKey::str_from(&name.0)).is_some());
            Ok(Value::bool_(found))
        }

        fn get_nonstandard_attr(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let rest = _interp.get_attr(self_val.0, "_rest")?;
            if let Some(d) = rest.as_dict()
                && let Some(v) = d.get(&PyKey::str_from(&name.0))
            {
                return Ok(v.clone());
            }
            Ok(default.as_value().cloned().unwrap_or_else(Value::none))
        }

        fn set_nonstandard_attr(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            value: PyValue,
        ) -> Result<Value> {
            let rest = _interp.get_attr(self_val.0, "_rest")?;
            rest.dict_insert(PyKey::str_from(&name.0), value.0)?;
            Ok(Value::none())
        }

        fn is_expired(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] now: OptValueRef<'_>,
        ) -> Result<Value> {
            let expires = _interp.get_attr(self_val.0, "expires")?;
            if expires.is_none() {
                return Ok(Value::bool_(false));
            }
            let now_ts = now
                .as_value()
                .filter(|v| !v.is_none())
                .and_then(value_as_f64)
                .unwrap_or_else(now_unix_secs);
            let exp_ts = value_as_f64(&expires);
            Ok(Value::bool_(exp_ts.is_some_and(|e| e <= now_ts)))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let name = _interp
                .get_attr(self_val.0, "name")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let val = _interp.get_attr(self_val.0, "value")?;
            let domain = _interp
                .get_attr(self_val.0, "domain")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let port = _interp.get_attr(self_val.0, "port")?;
            let path = _interp
                .get_attr(self_val.0, "path")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let nv = if let Some(vs) = val.as_str() {
                format!("{name}={vs}")
            } else {
                name
            };
            let dom_port = if let Some(ps) = port.as_str() {
                format!("{domain}:{ps}")
            } else {
                domain
            };
            Ok(Value::string(format!("<Cookie {nv} for {dom_port}{path}>")))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let fields = [
                ("version", "version"),
                ("name", "name"),
                ("value", "value"),
                ("port", "port"),
                ("port_specified", "port_specified"),
                ("domain", "domain"),
                ("domain_specified", "domain_specified"),
                ("domain_initial_dot", "domain_initial_dot"),
                ("path", "path"),
                ("path_specified", "path_specified"),
                ("secure", "secure"),
                ("expires", "expires"),
                ("discard", "discard"),
                ("comment", "comment"),
                ("comment_url", "comment_url"),
                ("rest", "_rest"),
                ("rfc2109", "rfc2109"),
            ];
            let mut parts = Vec::with_capacity(fields.len());
            for (label, attr) in fields {
                let v = _interp.get_attr(self_val.0, attr)?;
                let r = render_value_repr(_interp, &v)?;
                parts.push(format!("{label}={r}"));
            }
            Ok(Value::string(format!("Cookie({})", parts.join(", "))))
        }
    }

    // ── CookiePolicy & DefaultCookiePolicy ───────────────────────────────────

    class CookiePolicy {
        constants {
            "netscape" => Value::bool_(true),
            "rfc2965" => Value::bool_(false),
            "hide_cookie2" => Value::bool_(false),
        }

        fn set_ok(
            #[positional_only] _self: PyValueRef<'_>,
            _cookie: PyValue,
            _request: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "set_ok"))
        }

        fn return_ok(
            #[positional_only] _self: PyValueRef<'_>,
            _cookie: PyValue,
            _request: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "return_ok"))
        }

        fn domain_return_ok(
            #[positional_only] _self: PyValueRef<'_>,
            _domain: PyValue,
            _request: PyValue,
        ) -> Result<Value> {
            Ok(Value::bool_(true))
        }

        fn path_return_ok(
            #[positional_only] _self: PyValueRef<'_>,
            _path: PyValue,
            _request: PyValue,
        ) -> Result<Value> {
            Ok(Value::bool_(true))
        }
    }

    class DefaultCookiePolicy("CookiePolicy") {
        constants {
            "DomainStrictNoDots" => Value::int(1),
            "DomainStrictNonDomain" => Value::int(2),
            "DomainRFC2965Match" => Value::int(4),
            "DomainLiberal" => Value::int(0),
            "DomainStrict" => Value::int(3),
        }

        #[allow(clippy::too_many_arguments)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] blocked_domains: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] allowed_domains: OptValueRef<'_>,
            #[default(true.into())] netscape: PyBool,
            #[default(false.into())] rfc2965: PyBool,
            #[default(OptionalArg::Omitted)] rfc2109_as_netscape: OptValueRef<'_>,
            #[default(false.into())] hide_cookie2: PyBool,
            #[default(false.into())] strict_domain: PyBool,
            #[default(true.into())] strict_rfc2965_unverifiable: PyBool,
            #[default(false.into())] strict_ns_unverifiable: PyBool,
            #[default(PyInt::from(0i64))] strict_ns_domain: PyInt,
            #[default(false.into())] strict_ns_set_initial_dollar: PyBool,
            #[default(false.into())] strict_ns_set_path: PyBool,
            #[default(OptionalArg::Omitted)] secure_protocols: OptValueRef<'_>,
        ) -> Result<Value> {
            let blocked = match blocked_domains.as_value() {
                Some(v) if !v.is_none() => Value::tuple(_interp.collect_iterable(v)?),
                _ => Value::tuple(Vec::new()),
            };
            let allowed = match allowed_domains.as_value() {
                Some(v) if !v.is_none() => Value::tuple(_interp.collect_iterable(v)?),
                _ => Value::none(),
            };
            let sec_protos = match secure_protocols.as_value() {
                Some(v) if !v.is_none() => Value::tuple(_interp.collect_iterable(v)?),
                _ => Value::tuple(vec![Value::string("https"), Value::string("wss")]),
            };
            let r2109 = match rfc2109_as_netscape.as_value() {
                Some(v) => v.clone(),
                None => Value::none(),
            };
            let ns_dom = strict_ns_domain.expect_i64(FN_NAME, "strict_ns_domain")?;
            let attrs: [(&str, Value); 14] = [
                ("_blocked_domains", blocked),
                ("_allowed_domains", allowed),
                ("netscape", Value::bool_(netscape.0)),
                ("rfc2965", Value::bool_(rfc2965.0)),
                ("rfc2109_as_netscape", r2109),
                ("hide_cookie2", Value::bool_(hide_cookie2.0)),
                ("strict_domain", Value::bool_(strict_domain.0)),
                (
                    "strict_rfc2965_unverifiable",
                    Value::bool_(strict_rfc2965_unverifiable.0),
                ),
                (
                    "strict_ns_unverifiable",
                    Value::bool_(strict_ns_unverifiable.0),
                ),
                ("strict_ns_domain", Value::int(ns_dom)),
                (
                    "strict_ns_set_initial_dollar",
                    Value::bool_(strict_ns_set_initial_dollar.0),
                ),
                ("strict_ns_set_path", Value::bool_(strict_ns_set_path.0)),
                ("secure_protocols", sec_protos),
                ("_now", Value::int(now_unix_secs().floor() as i64)),
            ];
            for (k, v) in attrs {
                _interp.assign_attr(self_val.0.clone(), k, v)?;
            }
            Ok(Value::none())
        }

        fn blocked_domains(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_blocked_domains")
        }

        fn set_blocked_domains(
            #[positional_only] self_val: PyValueRef<'_>,
            blocked_domains: PyValue,
        ) -> Result<Value> {
            let t = Value::tuple(_interp.collect_iterable(&blocked_domains.0)?);
            _interp.assign_attr(self_val.0.clone(), "_blocked_domains", t)?;
            Ok(Value::none())
        }

        fn is_blocked(
            #[positional_only] self_val: PyValueRef<'_>,
            domain: PyStr<'_>,
        ) -> Result<Value> {
            let blocked = _interp.get_attr(self_val.0, "_blocked_domains")?;
            for item in _interp.collect_iterable(&blocked)? {
                if let Some(d) = item.as_str()
                    && user_domain_match_str(&domain.0, d)
                {
                    return Ok(Value::bool_(true));
                }
            }
            Ok(Value::bool_(false))
        }

        fn allowed_domains(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_allowed_domains")
        }

        fn set_allowed_domains(
            #[positional_only] self_val: PyValueRef<'_>,
            allowed_domains: PyValue,
        ) -> Result<Value> {
            let v = if allowed_domains.0.is_none() {
                Value::none()
            } else {
                Value::tuple(_interp.collect_iterable(&allowed_domains.0)?)
            };
            _interp.assign_attr(self_val.0.clone(), "_allowed_domains", v)?;
            Ok(Value::none())
        }

        fn is_not_allowed(
            #[positional_only] self_val: PyValueRef<'_>,
            domain: PyStr<'_>,
        ) -> Result<Value> {
            let allowed = _interp.get_attr(self_val.0, "_allowed_domains")?;
            if allowed.is_none() {
                return Ok(Value::bool_(false));
            }
            for item in _interp.collect_iterable(&allowed)? {
                if let Some(d) = item.as_str()
                    && user_domain_match_str(&domain.0, d)
                {
                    return Ok(Value::bool_(false));
                }
            }
            Ok(Value::bool_(true))
        }

        fn set_ok_version(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            _request: PyValue,
        ) -> Result<Value> {
            let ver = _interp.get_attr(&cookie.0, "version")?.as_int().unwrap_or(0);
            if ver == 0 && !get_attr_truthy(_interp, self_val.0, "netscape")? {
                return Ok(Value::bool_(false));
            }
            if ver > 0 && !get_attr_truthy(_interp, self_val.0, "rfc2965")? {
                let rfc2109 = get_attr_truthy(_interp, &cookie.0, "rfc2109")?;
                let as_ns = _interp.get_attr(self_val.0, "rfc2109_as_netscape")?;
                let allow_2109 = rfc2109
                    && (as_ns.is_none() || _interp.truthy_value(&as_ns)?)
                    && get_attr_truthy(_interp, self_val.0, "netscape")?;
                if !allow_2109 {
                    return Ok(Value::bool_(false));
                }
            }
            Ok(Value::bool_(true))
        }

        fn set_ok_verifiability(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            if request_unverifiable_bool(_interp, &request.0)? {
                let ver = _interp.get_attr(&cookie.0, "version")?.as_int().unwrap_or(0);
                let strict = if ver == 0 {
                    get_attr_truthy(_interp, self_val.0, "strict_ns_unverifiable")?
                } else {
                    get_attr_truthy(_interp, self_val.0, "strict_rfc2965_unverifiable")?
                };
                if strict {
                    let origin = request_origin_host_str(_interp, &request.0)?;
                    let host = request_host_str(_interp, &request.0)?;
                    if origin != host && !domain_match_str(&host, &format!(".{origin}")) {
                        return Ok(Value::bool_(false));
                    }
                }
            }
            Ok(Value::bool_(true))
        }

        fn set_ok_name(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            _request: PyValue,
        ) -> Result<Value> {
            let ver = _interp.get_attr(&cookie.0, "version")?.as_int().unwrap_or(0);
            if ver == 0
                && get_attr_truthy(_interp, self_val.0, "strict_ns_set_initial_dollar")?
                && _interp
                    .get_attr(&cookie.0, "name")?
                    .as_str()
                    .is_some_and(|n| n.starts_with('$'))
            {
                return Ok(Value::bool_(false));
            }
            Ok(Value::bool_(true))
        }

        fn set_ok_path(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            let path_spec = get_attr_truthy(_interp, &cookie.0, "path_specified")?;
            if path_spec {
                let ver = _interp.get_attr(&cookie.0, "version")?.as_int().unwrap_or(0);
                let check =
                    ver > 0 || get_attr_truthy(_interp, self_val.0, "strict_ns_set_path")?;
                if check {
                    let req_path = request_path_str(_interp, &request.0)?;
                    let c_path = _interp
                        .get_attr(&cookie.0, "path")?
                        .as_str()
                        .unwrap_or("/")
                        .to_string();
                    if !req_path.starts_with(&c_path) {
                        return Ok(Value::bool_(false));
                    }
                }
            }
            Ok(Value::bool_(true))
        }

        fn set_ok_domain(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            let domain = _interp
                .get_attr(&cookie.0, "domain")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let blk = call_method(_interp, self_val.0, "is_blocked", &[Value::string(&domain)])?;
            if _interp.truthy_value(&blk)? {
                return Ok(Value::bool_(false));
            }
            let not_alw =
                call_method(_interp, self_val.0, "is_not_allowed", &[Value::string(&domain)])?;
            if _interp.truthy_value(&not_alw)? {
                return Ok(Value::bool_(false));
            }
            let dom_spec = get_attr_truthy(_interp, &cookie.0, "domain_specified")?;
            if dom_spec {
                let (req_host, erhn) = eff_request_host_pair(_interp, &request.0)?;
                let strict_dom = get_attr_truthy(_interp, self_val.0, "strict_domain")?;
                if strict_dom && domain.matches('.').count() < 2 && domain != ".local" {
                    return Ok(Value::bool_(false));
                }
                if !domain_match_str(&req_host, &domain) && !domain_match_str(&erhn, &domain) {
                    return Ok(Value::bool_(false));
                }
                let ver = _interp.get_attr(&cookie.0, "version")?.as_int().unwrap_or(0);
                let ns_dom = _interp
                    .get_attr(self_val.0, "strict_ns_domain")?
                    .as_int()
                    .unwrap_or(0);
                if ver > 0 || (ns_dom & 4) != 0 {
                    let dom_no_dot = domain.trim_start_matches('.');
                    let host_prefix = req_host.strip_suffix(dom_no_dot).unwrap_or("");
                    if host_prefix.trim_end_matches('.').contains('.') {
                        return Ok(Value::bool_(false));
                    }
                }
            }
            Ok(Value::bool_(true))
        }

        fn set_ok_port(
            #[positional_only] _self: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            let port_spec = get_attr_truthy(_interp, &cookie.0, "port_specified")?;
            if port_spec {
                let req_port = request_port_str(_interp, &request.0)?;
                let c_port = _interp
                    .get_attr(&cookie.0, "port")?
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let matches = c_port.split(',').any(|p| p.trim() == req_port);
                return Ok(Value::bool_(matches));
            }
            Ok(Value::bool_(true))
        }

        fn set_ok(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            for check in [
                "set_ok_version",
                "set_ok_verifiability",
                "set_ok_name",
                "set_ok_path",
                "set_ok_domain",
                "set_ok_port",
            ] {
                let res = call_method(
                    _interp,
                    self_val.0,
                    check,
                    &[cookie.0.clone(), request.0.clone()],
                )?;
                if !_interp.truthy_value(&res)? {
                    return Ok(Value::bool_(false));
                }
            }
            Ok(Value::bool_(true))
        }

        fn return_ok_version(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            _request: PyValue,
        ) -> Result<Value> {
            let ver = _interp.get_attr(&cookie.0, "version")?.as_int().unwrap_or(0);
            if ver == 0 && !get_attr_truthy(_interp, self_val.0, "netscape")? {
                return Ok(Value::bool_(false));
            }
            if ver > 0 && !get_attr_truthy(_interp, self_val.0, "rfc2965")? {
                return Ok(Value::bool_(false));
            }
            Ok(Value::bool_(true))
        }

        fn return_ok_verifiability(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "set_ok_verifiability", &[cookie.0, request.0])
        }

        fn return_ok_secure(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            let secure = get_attr_truthy(_interp, &cookie.0, "secure")?;
            if secure {
                let scheme = request_scheme_str(_interp, &request.0)?;
                let protos = _interp.get_attr(self_val.0, "secure_protocols")?;
                let ok = _interp
                    .collect_iterable(&protos)?
                    .iter()
                    .any(|p| p.as_str().is_some_and(|s| s.eq_ignore_ascii_case(&scheme)));
                return Ok(Value::bool_(ok));
            }
            Ok(Value::bool_(true))
        }

        fn return_ok_expires(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            _request: PyValue,
        ) -> Result<Value> {
            let now_val = _interp
                .get_attr(self_val.0, "_now")
                .unwrap_or_else(|_| Value::float(now_unix_secs()));
            let expired = call_method(_interp, &cookie.0, "is_expired", &[now_val])?;
            Ok(Value::bool_(!_interp.truthy_value(&expired)?))
        }

        fn return_ok_port(
            #[positional_only] _self: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            let port_val = _interp.get_attr(&cookie.0, "port")?;
            if let Some(c_port) = port_val.as_str() {
                let req_port = request_port_str(_interp, &request.0)?;
                let ok = c_port.split(',').any(|p| p.trim() == req_port);
                return Ok(Value::bool_(ok));
            }
            Ok(Value::bool_(true))
        }

        fn return_ok_domain(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            let domain = _interp.get_attr(&cookie.0, "domain")?;
            call_method(_interp, self_val.0, "domain_return_ok", &[domain, request.0])
        }

        fn return_ok(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            for check in [
                "return_ok_version",
                "return_ok_verifiability",
                "return_ok_secure",
                "return_ok_expires",
                "return_ok_port",
                "return_ok_domain",
            ] {
                let res = call_method(
                    _interp,
                    self_val.0,
                    check,
                    &[cookie.0.clone(), request.0.clone()],
                )?;
                if !_interp.truthy_value(&res)? {
                    return Ok(Value::bool_(false));
                }
            }
            Ok(Value::bool_(true))
        }

        fn domain_return_ok(
            #[positional_only] self_val: PyValueRef<'_>,
            domain: PyStr<'_>,
            request: PyValue,
        ) -> Result<Value> {
            let (req_host, erhn) = eff_request_host_pair(_interp, &request.0)?;
            let dom = domain.0.to_ascii_lowercase();
            let req_dot = format!(".{erhn}");
            let matched = if dom.starts_with('.') {
                domain_match_str(&req_host, &dom) || req_dot == dom
            } else {
                req_host == dom || erhn == dom
            };
            if !matched {
                return Ok(Value::bool_(false));
            }
            let blk = call_method(_interp, self_val.0, "is_blocked", &[Value::string(&dom)])?;
            if _interp.truthy_value(&blk)? {
                return Ok(Value::bool_(false));
            }
            let not_alw =
                call_method(_interp, self_val.0, "is_not_allowed", &[Value::string(&dom)])?;
            if _interp.truthy_value(&not_alw)? {
                return Ok(Value::bool_(false));
            }
            Ok(Value::bool_(true))
        }

        fn path_return_ok(
            #[positional_only] _self: PyValueRef<'_>,
            path: PyStr<'_>,
            request: PyValue,
        ) -> Result<Value> {
            let req_path = request_path_str(_interp, &request.0)?;
            Ok(Value::bool_(req_path.starts_with(path.0.as_ref())))
        }
    }

    // ── CookieJar ────────────────────────────────────────────────────────────

    class CookieJar {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] policy: OptValueRef<'_>,
        ) -> Result<Value> {
            let pol = match policy.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let dcp = cj_attr(_interp, "DefaultCookiePolicy")?;
                    call_pos(_interp, dcp, &[])?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "_policy", pol)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_cookies",
                Value::dict(PyDict::default()),
            )?;
            Ok(Value::none())
        }

        fn set_policy(
            #[positional_only] self_val: PyValueRef<'_>,
            policy: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_policy", policy.0)?;
            Ok(Value::none())
        }

        fn set_cookie(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
        ) -> Result<Value> {
            let domain = _interp
                .get_attr(&cookie.0, "domain")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let path = _interp
                .get_attr(&cookie.0, "path")?
                .as_str()
                .unwrap_or("/")
                .to_string();
            let name = _interp
                .get_attr(&cookie.0, "name")?
                .as_str()
                .unwrap_or_default()
                .to_string();

            let cookies = _interp.get_attr(self_val.0, "_cookies")?;
            let dom_dict = {
                let existing = cookies
                    .as_dict()
                    .and_then(|d| d.get(&PyKey::str_from(&domain)).cloned());
                match existing {
                    Some(v) => v,
                    None => {
                        let nd = Value::dict(PyDict::default());
                        cookies.dict_insert(PyKey::str_from(&domain), nd.clone())?;
                        nd
                    }
                }
            };
            let path_dict = {
                let existing = dom_dict
                    .as_dict()
                    .and_then(|d| d.get(&PyKey::str_from(&path)).cloned());
                match existing {
                    Some(v) => v,
                    None => {
                        let nd = Value::dict(PyDict::default());
                        dom_dict.dict_insert(PyKey::str_from(&path), nd.clone())?;
                        nd
                    }
                }
            };
            path_dict.dict_insert(PyKey::str_from(&name), cookie.0)?;
            Ok(Value::none())
        }

        fn set_cookie_if_ok(
            #[positional_only] self_val: PyValueRef<'_>,
            cookie: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            let policy = _interp.get_attr(self_val.0, "_policy")?;
            let _ = _interp.assign_attr(
                policy.clone(),
                "_now",
                Value::int(now_unix_secs().floor() as i64),
            );
            let ok = call_method(
                _interp,
                &policy,
                "set_ok",
                &[cookie.0.clone(), request.0],
            )?;
            if _interp.truthy_value(&ok)? {
                call_method(_interp, self_val.0, "set_cookie", &[cookie.0])?;
            }
            Ok(Value::none())
        }

        fn make_cookies(
            #[positional_only] _self: PyValueRef<'_>,
            response: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            let (ns_headers, rfc_headers) = extract_set_cookie_headers(_interp, &response.0)?;
            let mut cookies = Vec::new();
            for h in ns_headers {
                if let Some(c) = cookie_from_set_cookie_header(_interp, &h, &request.0, false)? {
                    cookies.push(c);
                }
            }
            for h in rfc_headers {
                if let Some(c) = cookie_from_set_cookie_header(_interp, &h, &request.0, true)? {
                    cookies.push(c);
                }
            }
            Ok(Value::list(cookies))
        }

        fn extract_cookies(
            #[positional_only] self_val: PyValueRef<'_>,
            response: PyValue,
            request: PyValue,
        ) -> Result<Value> {
            let policy = _interp.get_attr(self_val.0, "_policy")?;
            let now = now_unix_secs().floor() as i64;
            let _ = _interp.assign_attr(policy.clone(), "_now", Value::int(now));
            let cookies_val = call_method(
                _interp,
                self_val.0,
                "make_cookies",
                &[response.0, request.0.clone()],
            )?;
            for cookie in _interp.collect_iterable(&cookies_val)? {
                let ok = call_method(
                    _interp,
                    &policy,
                    "set_ok",
                    &[cookie.clone(), request.0.clone()],
                )?;
                if _interp.truthy_value(&ok)? {
                    let expired =
                        call_method(_interp, &cookie, "is_expired", &[Value::int(now)])?;
                    if _interp.truthy_value(&expired)? {
                        let d = _interp.get_attr(&cookie, "domain")?;
                        let p = _interp.get_attr(&cookie, "path")?;
                        let n = _interp.get_attr(&cookie, "name")?;
                        let _ = call_method(_interp, self_val.0, "clear", &[d, p, n]);
                    } else {
                        call_method(_interp, self_val.0, "set_cookie", &[cookie])?;
                    }
                }
            }
            Ok(Value::none())
        }

        fn add_cookie_header(
            #[positional_only] self_val: PyValueRef<'_>,
            request: PyValue,
        ) -> Result<Value> {
            let policy = _interp.get_attr(self_val.0, "_policy")?;
            let now = now_unix_secs().floor() as i64;
            let _ = _interp.assign_attr(policy.clone(), "_now", Value::int(now));

            let all_cookies = collect_all_cookies_sorted(_interp, self_val.0)?;
            let mut matched: Vec<(usize, Value)> = Vec::new();
            for cookie in all_cookies {
                let domain = _interp.get_attr(&cookie, "domain")?;
                let path = _interp.get_attr(&cookie, "path")?;
                let dom_ok = call_method(
                    _interp,
                    &policy,
                    "domain_return_ok",
                    &[domain, request.0.clone()],
                )?;
                if !_interp.truthy_value(&dom_ok)? {
                    continue;
                }
                let path_ok = call_method(
                    _interp,
                    &policy,
                    "path_return_ok",
                    &[path.clone(), request.0.clone()],
                )?;
                if !_interp.truthy_value(&path_ok)? {
                    continue;
                }
                let ret_ok = call_method(
                    _interp,
                    &policy,
                    "return_ok",
                    &[cookie.clone(), request.0.clone()],
                )?;
                if !_interp.truthy_value(&ret_ok)? {
                    continue;
                }
                let plen = path.as_str().map(str::len).unwrap_or(0);
                matched.push((plen, cookie));
            }
            // Longer paths first (RFC 6265 / CPython ordering).
            matched.sort_by_key(|a| std::cmp::Reverse(a.0));

            let mut pairs: Vec<String> = Vec::with_capacity(matched.len());
            for (_, cookie) in matched {
                let name = _interp
                    .get_attr(&cookie, "name")?
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let val = _interp.get_attr(&cookie, "value")?;
                if let Some(vs) = val.as_str() {
                    pairs.push(format!("{name}={vs}"));
                } else {
                    pairs.push(name);
                }
            }
            if !pairs.is_empty() {
                let has_cookie = if let Ok(hh) = _interp.get_attr(&request.0, "has_header") {
                    let r = call_pos(_interp, hh, &[Value::string("Cookie")])?;
                    _interp.truthy_value(&r)?
                } else {
                    false
                };
                if !has_cookie {
                    let header_val = pairs.join("; ");
                    call_method(
                        _interp,
                        &request.0,
                        "add_unredirected_header",
                        &[Value::string("Cookie"), Value::string(header_val)],
                    )?;
                }
            }
            Ok(Value::none())
        }

        fn clear(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] domain: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] path: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
        ) -> Result<Value> {
            let d_opt = domain.as_value().filter(|v| !v.is_none());
            let p_opt = path.as_value().filter(|v| !v.is_none());
            let n_opt = name.as_value().filter(|v| !v.is_none());

            if n_opt.is_some() && (d_opt.is_none() || p_opt.is_none()) {
                return Err(PyError::named(
                    "ValueError",
                    "domain and path must be specified to remove a cookie by name",
                ));
            }
            if p_opt.is_some() && d_opt.is_none() {
                return Err(PyError::named(
                    "ValueError",
                    "domain must be specified to remove cookies by path",
                ));
            }
            let cookies = _interp.get_attr(self_val.0, "_cookies")?;
            let Some(d_val) = d_opt else {
                _interp.assign_attr(
                    self_val.0.clone(),
                    "_cookies",
                    Value::dict(PyDict::default()),
                )?;
                return Ok(Value::none());
            };
            let d_str = d_val.as_str().unwrap_or_default().to_string();
            let d_key = PyKey::str_from(&d_str);
            let Some(p_val) = p_opt else {
                let removed = cookies.dict_shift_remove(&d_key)?;
                if removed.is_none() {
                    return Err(PyError::named("KeyError", format!("{d_str:?}")));
                }
                return Ok(Value::none());
            };
            let p_str = p_val.as_str().unwrap_or_default().to_string();
            let p_key = PyKey::str_from(&p_str);
            let dom_dict = cookies
                .as_dict()
                .and_then(|d| d.get(&d_key).cloned())
                .ok_or_else(|| PyError::named("KeyError", format!("{d_str:?}")))?;
            let Some(n_val) = n_opt else {
                let removed = dom_dict.dict_shift_remove(&p_key)?;
                if removed.is_none() {
                    return Err(PyError::named("KeyError", format!("{p_str:?}")));
                }
                return Ok(Value::none());
            };
            let n_str = n_val.as_str().unwrap_or_default().to_string();
            let n_key = PyKey::str_from(&n_str);
            let path_dict = dom_dict
                .as_dict()
                .and_then(|d| d.get(&p_key).cloned())
                .ok_or_else(|| PyError::named("KeyError", format!("{p_str:?}")))?;
            let removed = path_dict.dict_shift_remove(&n_key)?;
            if removed.is_none() {
                return Err(PyError::named("KeyError", format!("{n_str:?}")));
            }
            Ok(Value::none())
        }

        fn clear_session_cookies(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let all = collect_all_cookies_sorted(_interp, self_val.0)?;
            let mut keep = Vec::new();
            for c in all {
                if !get_attr_truthy(_interp, &c, "discard")? {
                    keep.push(c);
                }
            }
            _interp.assign_attr(
                self_val.0.clone(),
                "_cookies",
                Value::dict(PyDict::default()),
            )?;
            for c in keep {
                call_method(_interp, self_val.0, "set_cookie", &[c])?;
            }
            Ok(Value::none())
        }

        fn clear_expired_cookies(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let now = now_unix_secs();
            let all = collect_all_cookies_sorted(_interp, self_val.0)?;
            let mut keep = Vec::new();
            for c in all {
                let expired = call_method(_interp, &c, "is_expired", &[Value::float(now)])?;
                if !_interp.truthy_value(&expired)? {
                    keep.push(c);
                }
            }
            _interp.assign_attr(
                self_val.0.clone(),
                "_cookies",
                Value::dict(PyDict::default()),
            )?;
            for c in keep {
                call_method(_interp, self_val.0, "set_cookie", &[c])?;
            }
            Ok(Value::none())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let all = collect_all_cookies_sorted(_interp, self_val.0)?;
            call_pos(_interp, Value::builtin_function("iter"), &[Value::list(all)])
        }

        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let all = collect_all_cookies_sorted(_interp, self_val.0)?;
            Ok(Value::int(all.len() as i64))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = value_class(self_val.0);
            let cls_name = _interp
                .get_attr(&cls, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "CookieJar".to_string());
            let all = collect_all_cookies_sorted(_interp, self_val.0)?;
            let mut parts = Vec::with_capacity(all.len());
            for c in &all {
                let s = call_pos(_interp, Value::builtin_function("str"), std::slice::from_ref(c))?;
                parts.push(s.as_str().unwrap_or_default().to_string());
            }
            Ok(Value::string(format!("<{cls_name}[{}]>", parts.join(", "))))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "__repr__", &[])
        }
    }

    // ── FileCookieJar ────────────────────────────────────────────────────────

    class FileCookieJar("CookieJar") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
            #[default(false.into())] delayload: PyBool,
            #[default(OptionalArg::Omitted)] policy: OptValueRef<'_>,
        ) -> Result<Value> {
            let pol_arg = match policy.as_value() {
                Some(v) => v.clone(),
                None => Value::none(),
            };
            let cj_cls = cj_attr(_interp, "CookieJar")?;
            let cj_init = _interp.get_attr(&cj_cls, "__init__")?;
            call_pos(_interp, cj_init, &[self_val.0.clone(), pol_arg])?;

            let fname_val = match filename.as_value() {
                Some(v) if !v.is_none() => {
                    if v.as_str().is_some() {
                        v.clone()
                    } else if let Ok(fspath_fn) = _interp.get_attr(v, "__fspath__") {
                        call_pos(_interp, fspath_fn, &[])?
                    } else {
                        return Err(PyError::named(
                            "TypeError",
                            "filename must be a string or PathLike object",
                        ));
                    }
                }
                _ => Value::none(),
            };
            _interp.assign_attr(self_val.0.clone(), "filename", fname_val)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "delayload",
                Value::bool_(delayload.0),
            )?;
            Ok(Value::none())
        }

        fn save(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _filename: OptValueRef<'_>,
            #[default(false.into())] _ignore_discard: PyBool,
            #[default(false.into())] _ignore_expires: PyBool,
        ) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "FileCookieJar.save() is abstract",
            ))
        }

        fn load(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
            #[default(false.into())] ignore_discard: PyBool,
            #[default(false.into())] ignore_expires: PyBool,
        ) -> Result<Value> {
            let fname = resolve_jar_filename(_interp, self_val.0, filename)?;
            let f = call_pos(
                _interp,
                Value::builtin_function("open"),
                &[Value::string(&fname), Value::string("r")],
            )?;
            let res = call_method(
                _interp,
                self_val.0,
                "_really_load",
                &[
                    f.clone(),
                    Value::string(&fname),
                    Value::bool_(ignore_discard.0),
                    Value::bool_(ignore_expires.0),
                ],
            );
            let _ = call_method(_interp, &f, "close", &[]);
            res?;
            Ok(Value::none())
        }

        fn revert(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
            #[default(false.into())] ignore_discard: PyBool,
            #[default(false.into())] ignore_expires: PyBool,
        ) -> Result<Value> {
            let backup = snapshot_cookies_dict(&_interp.get_attr(self_val.0, "_cookies")?);
            call_method(_interp, self_val.0, "clear", &[])?;
            let fname_arg = filename.as_value().cloned().unwrap_or_else(Value::none);
            let res = call_method(
                _interp,
                self_val.0,
                "load",
                &[
                    fname_arg,
                    Value::bool_(ignore_discard.0),
                    Value::bool_(ignore_expires.0),
                ],
            );
            if let Err(err) = res {
                let _ = _interp.assign_attr(self_val.0.clone(), "_cookies", backup);
                return Err(err);
            }
            Ok(Value::none())
        }
    }

    // ── MozillaCookieJar ─────────────────────────────────────────────────────

    class MozillaCookieJar("FileCookieJar") {
        constants {
            "magic_re" => Value::string("#( Netscape)? HTTP Cookie File"),
            "header" => Value::string(
                "# Netscape HTTP Cookie File\n# https://curl.haxx.se/rfc/cookie_spec.html\n# This is a generated file!  Do not edit.\n\n"
            ),
        }

        fn save(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
            #[default(false.into())] ignore_discard: PyBool,
            #[default(false.into())] ignore_expires: PyBool,
        ) -> Result<Value> {
            let fname = resolve_jar_filename(_interp, self_val.0, filename)?;
            let header = _interp
                .get_attr(self_val.0, "header")?
                .as_str()
                .unwrap_or("# Netscape HTTP Cookie File\n\n")
                .to_string();
            let now = now_unix_secs();
            let mut out = String::from(&header);
            for cookie in collect_all_cookies_sorted(_interp, self_val.0)? {
                if should_skip_cookie(_interp, &cookie, ignore_discard.0, ignore_expires.0, now)? {
                    continue;
                }
                let domain = _interp
                    .get_attr(&cookie, "domain")?
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let initial_dot = if domain.starts_with('.') {
                    "TRUE"
                } else {
                    "FALSE"
                };
                let path = _interp
                    .get_attr(&cookie, "path")?
                    .as_str()
                    .unwrap_or("/")
                    .to_string();
                let secure = if get_attr_truthy(_interp, &cookie, "secure")? {
                    "TRUE"
                } else {
                    "FALSE"
                };
                let expires_val = _interp.get_attr(&cookie, "expires")?;
                let expires_str = if expires_val.is_none() {
                    String::new()
                } else if let Some(i) = expires_val.as_int() {
                    i.to_string()
                } else if let Some(fl) = value_as_f64(&expires_val) {
                    (fl as i64).to_string()
                } else {
                    String::new()
                };
                let name = _interp
                    .get_attr(&cookie, "name")?
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let val_obj = _interp.get_attr(&cookie, "value")?;
                let (col_name, col_val) = if let Some(vs) = val_obj.as_str() {
                    (name, vs.to_string())
                } else {
                    (String::new(), name)
                };
                out.push_str(&format!(
                    "{domain}\t{initial_dot}\t{path}\t{secure}\t{expires_str}\t{col_name}\t{col_val}\n"
                ));
            }
            std::fs::write(&fname, out.as_bytes())
                .map_err(|e| PyError::named("OSError", format!("{e}")))?;
            Ok(Value::none())
        }

        fn _really_load(
            #[positional_only] self_val: PyValueRef<'_>,
            f: PyValue,
            filename: PyPath,
            ignore_discard: PyBool,
            ignore_expires: PyBool,
        ) -> Result<Value> {
            let content = read_all_text_from_file(_interp, &f.0)?;
            let mut lines = content.lines();
            let first_line = lines.next().unwrap_or("");
            if !(first_line.starts_with("# Netscape HTTP Cookie File")
                || first_line.starts_with("# HTTP Cookie File"))
            {
                return Err(raise_load_error(
                    _interp,
                    format!(
                        "{:?} does not look like a Netscape format cookies file",
                        filename.as_path().display().to_string()
                    ),
                ));
            }
            let now = now_unix_secs();
            for raw_line in lines {
                let line = raw_line.trim_end_matches(['\r', '\n']);
                if line.trim().is_empty() {
                    continue;
                }
                let mut rest_dict = PyDict::default();
                let effective = if let Some(stripped) = line.strip_prefix("#HttpOnly_") {
                    rest_dict.insert(PyKey::str_from("HttpOnly"), Value::none());
                    stripped
                } else if line.trim_start().starts_with('#') {
                    continue;
                } else {
                    line
                };
                let cols: Vec<&str> = effective.split('\t').collect();
                if cols.len() < 6 {
                    return Err(raise_load_error(
                        _interp,
                        format!("invalid Netscape format cookies file: {line:?}"),
                    ));
                }
                let domain = cols[0].to_string();
                let domain_specified = cols[1].eq_ignore_ascii_case("TRUE");
                let initial_dot = domain.starts_with('.');
                let path = cols[2].to_string();
                let secure = cols[3].eq_ignore_ascii_case("TRUE");
                let (expires, discard) = if cols[4].is_empty() {
                    (Value::none(), true)
                } else if let Ok(ts) = cols[4].parse::<i64>() {
                    if ts == 0 {
                        (Value::none(), true)
                    } else {
                        (Value::int(ts), false)
                    }
                } else {
                    (Value::none(), true)
                };
                let (name, value) = if cols.len() >= 7 {
                    if cols[5].is_empty() {
                        (cols[6].to_string(), Value::none())
                    } else {
                        (cols[5].to_string(), Value::string(cols[6]))
                    }
                } else {
                    (cols[5].to_string(), Value::string(""))
                };
                let cookie = construct_cookie(
                    _interp,
                    0,
                    name,
                    value,
                    Value::none(),
                    false,
                    domain,
                    domain_specified,
                    initial_dot,
                    path,
                    !cols[2].is_empty(),
                    secure,
                    expires,
                    discard,
                    Value::none(),
                    Value::none(),
                    Value::dict(rest_dict),
                    false,
                )?;
                if should_skip_cookie(
                    _interp,
                    &cookie,
                    ignore_discard.0,
                    ignore_expires.0,
                    now,
                )? {
                    continue;
                }
                call_method(_interp, self_val.0, "set_cookie", &[cookie])?;
            }
            Ok(Value::none())
        }
    }

    // ── LWPCookieJar ─────────────────────────────────────────────────────────

    class LWPCookieJar("FileCookieJar") {
        constants {
            "magic_re" => Value::string("#LWP-Cookies-(\\d+\\.\\d+)"),
        }

        fn as_lwp_str(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(true.into())] ignore_discard: PyBool,
            #[default(true.into())] ignore_expires: PyBool,
        ) -> Result<Value> {
            let now = now_unix_secs();
            let mut lines = Vec::new();
            for cookie in collect_all_cookies_sorted(_interp, self_val.0)? {
                if should_skip_cookie(_interp, &cookie, ignore_discard.0, ignore_expires.0, now)? {
                    continue;
                }
                let s = format_lwp_cookie_str(_interp, &cookie)?;
                lines.push(format!("Set-Cookie3: {s}\n"));
            }
            Ok(Value::string(lines.concat()))
        }

        fn save(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
            #[default(false.into())] ignore_discard: PyBool,
            #[default(false.into())] ignore_expires: PyBool,
        ) -> Result<Value> {
            let fname = resolve_jar_filename(_interp, self_val.0, filename)?;
            let body = call_method(
                _interp,
                self_val.0,
                "as_lwp_str",
                &[Value::bool_(ignore_discard.0), Value::bool_(ignore_expires.0)],
            )?;
            let body_str = body.as_str().unwrap_or_default();
            let full = format!("#LWP-Cookies-2.0\n{body_str}");
            std::fs::write(&fname, full.as_bytes())
                .map_err(|e| PyError::named("OSError", format!("{e}")))?;
            Ok(Value::none())
        }

        fn _really_load(
            #[positional_only] self_val: PyValueRef<'_>,
            f: PyValue,
            filename: PyPath,
            ignore_discard: PyBool,
            ignore_expires: PyBool,
        ) -> Result<Value> {
            let content = read_all_text_from_file(_interp, &f.0)?;
            let mut lines = content.lines();
            let first_line = lines.next().unwrap_or("");
            if !first_line.starts_with("#LWP-Cookies-") {
                return Err(raise_load_error(
                    _interp,
                    format!(
                        "{:?} does not look like a Set-Cookie3 (LWP) format file",
                        filename.as_path().display().to_string()
                    ),
                ));
            }
            let now = now_unix_secs();
            for raw_line in lines {
                let line = raw_line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some(rest_str) = line.strip_prefix("Set-Cookie3:") else {
                    continue;
                };
                let pairs = parse_cookie_attrs_str(rest_str.trim());
                if pairs.is_empty() {
                    continue;
                }
                let (name, raw_val) = &pairs[0];
                let value = match raw_val {
                    Some(v) => Value::string(v.clone()),
                    None => Value::none(),
                };
                let mut version = 0i64;
                let mut port = Value::none();
                let mut port_specified = false;
                let mut domain = String::new();
                let mut domain_specified = false;
                let mut domain_initial_dot = false;
                let mut path = "/".to_string();
                let mut path_specified = false;
                let mut secure = false;
                let mut expires = Value::none();
                let mut discard = false;
                let mut comment = Value::none();
                let mut comment_url = Value::none();
                let mut rest_dict = PyDict::default();

                for (k, v_opt) in pairs.iter().skip(1) {
                    match k.as_str() {
                        "path" => {
                            if let Some(p) = v_opt {
                                path = p.clone();
                            }
                        }
                        "domain" => {
                            if let Some(d) = v_opt {
                                domain = d.clone();
                                domain_specified = domain.starts_with('.');
                            }
                        }
                        "port" => {
                            if let Some(p) = v_opt {
                                port = Value::string(p.clone());
                            }
                        }
                        "path_spec" => path_specified = true,
                        "port_spec" => port_specified = true,
                        "domain_dot" => {
                            domain_initial_dot = true;
                            domain_specified = true;
                        }
                        "secure" => secure = true,
                        "discard" => discard = true,
                        "expires" => {
                            if let Some(es) = v_opt
                                && let Some(ts) = parse_iso2time(es)
                            {
                                expires = Value::int(ts);
                            }
                        }
                        "comment" => {
                            if let Some(c) = v_opt {
                                comment = Value::string(c.clone());
                            }
                        }
                        "commenturl" => {
                            if let Some(cu) = v_opt {
                                comment_url = Value::string(cu.clone());
                            }
                        }
                        "version" => {
                            if let Some(vs) = v_opt
                                && let Ok(v) = vs.parse::<i64>()
                            {
                                version = v;
                            }
                        }
                        _ => {
                            let rv = match v_opt {
                                Some(v) => Value::string(v.clone()),
                                None => Value::none(),
                            };
                            rest_dict.insert(PyKey::str_from(k), rv);
                        }
                    }
                }
                let cookie = construct_cookie(
                    _interp,
                    version,
                    name.clone(),
                    value,
                    port,
                    port_specified,
                    domain,
                    domain_specified,
                    domain_initial_dot,
                    path,
                    path_specified,
                    secure,
                    expires,
                    discard,
                    comment,
                    comment_url,
                    Value::dict(rest_dict),
                    false,
                )?;
                if should_skip_cookie(
                    _interp,
                    &cookie,
                    ignore_discard.0,
                    ignore_expires.0,
                    now,
                )? {
                    continue;
                }
                call_method(_interp, self_val.0, "set_cookie", &[cookie])?;
            }
            Ok(Value::none())
        }
    }
}
