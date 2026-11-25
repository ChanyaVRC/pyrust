// `urllib.robotparser` module — 100% native-Rust Python 3.12 implementation
// using PyRust's expanded macro toolkit (`pyrust_module!`, `pyrust_struct_seq!`,
// `class`).
//
// Provides `RequestRate`, `RuleLine`, `Entry`, and `RobotFileParser` with full
// CPython 3.12 parity for `robots.txt` parsing (`User-agent`, `Disallow`,
// `Allow`, `Crawl-delay`, `Request-rate`, `Sitemap`, comments, HTTP status
// handling in `read()`, and `can_fetch()`).
//
// Reference: <https://docs.python.org/3/library/urllib.robotparser.html>

use std::time::{SystemTime, UNIX_EPOCH};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

pyrust_struct_seq! {
    pub(crate) RequestRate in "urllib.robotparser" {
        fields: [requests, seconds],
    }
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

fn rp_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("urllib.robotparser")?;
    interp.get_attr(&m, name)
}

fn now_unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn percent_unquote(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_quote(input: &str, safe: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        let is_unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~');
        if is_unreserved || (b < 128 && safe.as_bytes().contains(&b)) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Matches CPython's `urllib.robotparser`: normalize path and query component
/// via `quote(unquote(part))` preserving `/`.
fn normalize_robot_path(raw: &str) -> String {
    if raw == "*" || raw.is_empty() {
        return raw.to_string();
    }
    // Split off fragment if any, preserve path + optional `?query`.
    let no_frag = raw.split('#').next().unwrap_or(raw);
    if let Some((path, query)) = no_frag.split_once('?') {
        let q_path = percent_quote(&percent_unquote(path), "/");
        let q_query = percent_quote(&percent_unquote(query), "/=&+");
        format!("{q_path}?{q_query}")
    } else {
        percent_quote(&percent_unquote(no_frag), "/")
    }
}

/// Extract `(netloc, path, query)` from a URL, matching `urllib.parse.urlsplit`.
fn split_url_parts(url: &str) -> (String, String, Option<String>) {
    let no_frag = url.split('#').next().unwrap_or(url);
    let after_scheme = if let Some(idx) = no_frag.find("://") {
        let scheme = &no_frag[..idx];
        if !scheme.is_empty()
            && scheme
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        {
            &no_frag[idx + 3..]
        } else {
            no_frag
        }
    } else if let Some(rest) = no_frag.strip_prefix("//") {
        rest
    } else {
        let (path, query) = match no_frag.split_once('?') {
            Some((p, q)) => (p.to_string(), Some(q.to_string())),
            None => (no_frag.to_string(), None),
        };
        return (String::new(), path, query);
    };
    let (netloc, rest) = match after_scheme.find(['/', '?']) {
        Some(idx) => (&after_scheme[..idx], &after_scheme[idx..]),
        None => (after_scheme, ""),
    };
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (rest.to_string(), None),
    };
    (netloc.to_string(), path, query)
}

fn new_entry(interp: &mut Interpreter) -> Result<Value> {
    let cls = rp_attr(interp, "Entry")?;
    call_pos(interp, cls, &[])
}

fn new_ruleline(interp: &mut Interpreter, path: &str, allowance: bool) -> Result<Value> {
    let cls = rp_attr(interp, "RuleLine")?;
    call_pos(interp, cls, &[Value::string(path), Value::bool_(allowance)])
}

fn add_entry_to_parser(interp: &mut Interpreter, parser: &Value, entry: Value) -> Result<()> {
    let uas = interp.get_attr(&entry, "useragents")?;
    let items = interp.collect_iterable(&uas)?;
    let is_default = items.iter().any(|v| v.as_str() == Some("*"));
    if is_default {
        let cur_default = interp.get_attr(parser, "default_entry")?;
        if cur_default.is_none() {
            interp.assign_attr(parser.clone(), "default_entry", entry)?;
        }
    } else {
        let entries = interp.get_attr(parser, "entries")?;
        call_method(interp, &entries, "append", &[entry])?;
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![Value::string("RobotFileParser")]),
        "RequestRate" => RequestRate::class(),
    }

    class RuleLine {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            path: PyStr<'_>,
            allowance: PyBool,
        ) -> Result<Value> {
            let effective_allowance = if path.0.is_empty() && !allowance.0 {
                true
            } else {
                allowance.0
            };
            let norm = normalize_robot_path(&path.0);
            _interp.assign_attr(self_val.0.clone(), "path", Value::string(norm))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "allowance",
                Value::bool_(effective_allowance),
            )?;
            Ok(Value::none())
        }

        fn applies_to(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            let p = _interp
                .get_attr(self_val.0, "path")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            Ok(Value::bool_(p == "*" || filename.0.starts_with(&p)))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let allow = get_attr_truthy(_interp, self_val.0, "allowance")?;
            let p = _interp
                .get_attr(self_val.0, "path")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let kw = if allow { "Allow" } else { "Disallow" };
            Ok(Value::string(format!("{kw}: {p}")))
        }
    }

    class Entry {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "useragents", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "rulelines", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "delay", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "req_rate", Value::none())?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut ret: Vec<String> = Vec::new();
            let uas = _interp.get_attr(self_val.0, "useragents")?;
            for ua in _interp.collect_iterable(&uas)? {
                if let Some(s) = ua.as_str() {
                    ret.push(format!("User-agent: {s}"));
                }
            }
            let delay = _interp.get_attr(self_val.0, "delay")?;
            if !delay.is_none() {
                let s = call_pos(_interp, Value::builtin_function("str"), &[delay])?;
                ret.push(format!("Crawl-delay: {}", s.as_str().unwrap_or_default()));
            }
            let rate = _interp.get_attr(self_val.0, "req_rate")?;
            if !rate.is_none() {
                let reqs = _interp.get_attr(&rate, "requests")?;
                let secs = _interp.get_attr(&rate, "seconds")?;
                let rs = call_pos(_interp, Value::builtin_function("str"), &[reqs])?;
                let ss = call_pos(_interp, Value::builtin_function("str"), &[secs])?;
                ret.push(format!(
                    "Request-rate: {}/{}",
                    rs.as_str().unwrap_or_default(),
                    ss.as_str().unwrap_or_default()
                ));
            }
            let rls = _interp.get_attr(self_val.0, "rulelines")?;
            for rl in _interp.collect_iterable(&rls)? {
                let s = call_pos(_interp, Value::builtin_function("str"), &[rl])?;
                ret.push(s.as_str().unwrap_or_default().to_string());
            }
            Ok(Value::string(ret.join("\n")))
        }

        fn applies_to(
            #[positional_only] self_val: PyValueRef<'_>,
            useragent: PyStr<'_>,
        ) -> Result<Value> {
            let token = useragent
                .0
                .split('/')
                .next()
                .unwrap_or(&useragent.0)
                .to_ascii_lowercase();
            let uas = _interp.get_attr(self_val.0, "useragents")?;
            for ua in _interp.collect_iterable(&uas)? {
                if let Some(agent) = ua.as_str() {
                    if agent == "*" {
                        return Ok(Value::bool_(true));
                    }
                    if token.contains(&agent.to_ascii_lowercase()) {
                        return Ok(Value::bool_(true));
                    }
                }
            }
            Ok(Value::bool_(false))
        }

        fn allowance(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            let rls = _interp.get_attr(self_val.0, "rulelines")?;
            for rl in _interp.collect_iterable(&rls)? {
                let applies = call_method(
                    _interp,
                    &rl,
                    "applies_to",
                    &[Value::string(filename.0.as_ref())],
                )?;
                if _interp.truthy_value(&applies)? {
                    return _interp.get_attr(&rl, "allowance");
                }
            }
            Ok(Value::bool_(true))
        }
    }

    class RobotFileParser {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(PyStr(std::borrow::Cow::Borrowed("")))] url: PyStr<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "entries", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "sitemaps", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "default_entry", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "disallow_all", Value::bool_(false))?;
            _interp.assign_attr(self_val.0.clone(), "allow_all", Value::bool_(false))?;
            call_method(_interp, self_val.0, "set_url", &[Value::string(url.0.as_ref())])?;
            _interp.assign_attr(self_val.0.clone(), "last_checked", Value::int(0))?;
            Ok(Value::none())
        }

        fn mtime(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "last_checked")
        }

        fn modified(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "last_checked",
                Value::float(now_unix_secs()),
            )?;
            Ok(Value::none())
        }

        fn set_url(
            #[positional_only] self_val: PyValueRef<'_>,
            url: PyStr<'_>,
        ) -> Result<Value> {
            let (host, path, _) = split_url_parts(&url.0);
            _interp.assign_attr(self_val.0.clone(), "url", Value::string(url.0.as_ref()))?;
            _interp.assign_attr(self_val.0.clone(), "host", Value::string(host))?;
            _interp.assign_attr(self_val.0.clone(), "path", Value::string(path))?;
            Ok(Value::none())
        }

        fn read(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let url = _interp.get_attr(self_val.0, "url")?;
            let req_mod = _interp.load_module("urllib.request")?;
            let urlopen = _interp.get_attr(&req_mod, "urlopen")?;
            match call_pos(_interp, urlopen, &[url]) {
                Ok(resp) => {
                    let raw = call_method(_interp, &resp, "read", &[])?;
                    let _ = call_method(_interp, &resp, "close", &[]);
                    let text = match raw.kind() {
                        ValueKind::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                        _ => raw.as_str().unwrap_or_default().to_string(),
                    };
                    let lines: Vec<Value> = text.lines().map(Value::string).collect();
                    call_method(_interp, self_val.0, "parse", &[Value::list(lines)])?;
                    Ok(Value::none())
                }
                Err(err) => {
                    let exc = _interp.materialize_pyerror(err.clone())?;
                    let err_mod = _interp.load_module("urllib.error")?;
                    let http_err_cls = _interp.get_attr(&err_mod, "HTTPError")?;
                    let is_http_err = call_pos(
                        _interp,
                        Value::builtin_function("isinstance"),
                        &[exc.clone(), http_err_cls],
                    )?;
                    if _interp.truthy_value(&is_http_err)? {
                        let code = _interp
                            .get_attr(&exc, "code")
                            .ok()
                            .and_then(|v| v.as_int())
                            .unwrap_or(0);
                        if code == 401 || code == 403 {
                            _interp.assign_attr(
                                self_val.0.clone(),
                                "disallow_all",
                                Value::bool_(true),
                            )?;
                        } else if (400..500).contains(&code) {
                            _interp.assign_attr(
                                self_val.0.clone(),
                                "allow_all",
                                Value::bool_(true),
                            )?;
                        }
                        let _ = call_method(_interp, &exc, "close", &[]);
                        return Ok(Value::none());
                    }
                    Err(err)
                }
            }
        }

        fn _add_entry(
            #[positional_only] self_val: PyValueRef<'_>,
            entry: PyValue,
        ) -> Result<Value> {
            add_entry_to_parser(_interp, self_val.0, entry.0)?;
            Ok(Value::none())
        }

        fn parse(
            #[positional_only] self_val: PyValueRef<'_>,
            lines: PyValue,
        ) -> Result<Value> {
            let line_vals = _interp.collect_iterable(&lines.0)?;
            let mut state = 0u8;
            let mut entry = new_entry(_interp)?;
            call_method(_interp, self_val.0, "modified", &[])?;

            for line_val in line_vals {
                let raw = line_val.as_str().ok_or_else(|| {
                    PyError::named("TypeError", "parse() lines must be strings")
                })?;
                if raw.is_empty() {
                    if state == 1 {
                        entry = new_entry(_interp)?;
                        state = 0;
                    } else if state == 2 {
                        add_entry_to_parser(_interp, self_val.0, entry)?;
                        entry = new_entry(_interp)?;
                        state = 0;
                    }
                }
                let no_comment = match raw.find('#') {
                    Some(idx) => &raw[..idx],
                    None => raw,
                };
                let trimmed = no_comment.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Some((k_raw, v_raw)) = trimmed.split_once(':') {
                    let key = k_raw.trim().to_ascii_lowercase();
                    let val = v_raw.trim();
                    match key.as_str() {
                        "user-agent" => {
                            if state == 2 {
                                add_entry_to_parser(_interp, self_val.0, entry)?;
                                entry = new_entry(_interp)?;
                            }
                            let uas = _interp.get_attr(&entry, "useragents")?;
                            call_method(_interp, &uas, "append", &[Value::string(val)])?;
                            state = 1;
                        }
                        "disallow" => {
                            if state != 0 {
                                let rl = new_ruleline(_interp, val, false)?;
                                let rls = _interp.get_attr(&entry, "rulelines")?;
                                call_method(_interp, &rls, "append", &[rl])?;
                                state = 2;
                            }
                        }
                        "allow" => {
                            if state != 0 {
                                let rl = new_ruleline(_interp, val, true)?;
                                let rls = _interp.get_attr(&entry, "rulelines")?;
                                call_method(_interp, &rls, "append", &[rl])?;
                                state = 2;
                            }
                        }
                        "crawl-delay" => {
                            if state != 0 {
                                if !val.is_empty() && val.bytes().all(|b| b.is_ascii_digit())
                                    && let Ok(n) = val.parse::<i64>()
                                {
                                    _interp.assign_attr(entry.clone(), "delay", Value::int(n))?;
                                }
                                state = 2;
                            }
                        }
                        "request-rate" => {
                            if state != 0 {
                                let parts: Vec<&str> = val.split('/').collect();
                                if parts.len() == 2 {
                                    let p0 = parts[0].trim();
                                    let p1 = parts[1].trim();
                                    if !p0.is_empty()
                                        && !p1.is_empty()
                                        && p0.bytes().all(|b| b.is_ascii_digit())
                                        && p1.bytes().all(|b| b.is_ascii_digit())
                                        && let (Ok(reqs), Ok(secs)) =
                                            (p0.parse::<i64>(), p1.parse::<i64>())
                                    {
                                        let rr = RequestRate::make([
                                            Value::int(reqs),
                                            Value::int(secs),
                                        ]);
                                        _interp.assign_attr(entry.clone(), "req_rate", rr)?;
                                    }
                                }
                                state = 2;
                            }
                        }
                        "sitemap" => {
                            let sitemaps = _interp.get_attr(self_val.0, "sitemaps")?;
                            call_method(_interp, &sitemaps, "append", &[Value::string(val)])?;
                        }
                        _ => {}
                    }
                }
            }
            if state == 2 {
                add_entry_to_parser(_interp, self_val.0, entry)?;
            }
            Ok(Value::none())
        }

        fn can_fetch(
            #[positional_only] self_val: PyValueRef<'_>,
            useragent: PyStr<'_>,
            url: PyStr<'_>,
        ) -> Result<Value> {
            if get_attr_truthy(_interp, self_val.0, "disallow_all")? {
                return Ok(Value::bool_(false));
            }
            if get_attr_truthy(_interp, self_val.0, "allow_all")? {
                return Ok(Value::bool_(true));
            }
            if !get_attr_truthy(_interp, self_val.0, "last_checked")? {
                return Ok(Value::bool_(false));
            }
            let (_, path, query) = split_url_parts(&url.0);
            let combined = match query {
                Some(q) => format!("{path}?{q}"),
                None => path,
            };
            let mut norm = normalize_robot_path(&combined);
            if norm.is_empty() {
                norm = "/".to_string();
            }
            let entries = _interp.get_attr(self_val.0, "entries")?;
            for entry in _interp.collect_iterable(&entries)? {
                let applies = call_method(
                    _interp,
                    &entry,
                    "applies_to",
                    &[Value::string(useragent.0.as_ref())],
                )?;
                if _interp.truthy_value(&applies)? {
                    return call_method(_interp, &entry, "allowance", &[Value::string(&norm)]);
                }
            }
            let def = _interp.get_attr(self_val.0, "default_entry")?;
            if !def.is_none() {
                return call_method(_interp, &def, "allowance", &[Value::string(&norm)]);
            }
            Ok(Value::bool_(true))
        }

        fn crawl_delay(
            #[positional_only] self_val: PyValueRef<'_>,
            useragent: PyStr<'_>,
        ) -> Result<Value> {
            let mt = call_method(_interp, self_val.0, "mtime", &[])?;
            if !_interp.truthy_value(&mt)? {
                return Ok(Value::none());
            }
            let entries = _interp.get_attr(self_val.0, "entries")?;
            for entry in _interp.collect_iterable(&entries)? {
                let applies = call_method(
                    _interp,
                    &entry,
                    "applies_to",
                    &[Value::string(useragent.0.as_ref())],
                )?;
                if _interp.truthy_value(&applies)? {
                    return _interp.get_attr(&entry, "delay");
                }
            }
            let def = _interp.get_attr(self_val.0, "default_entry")?;
            if !def.is_none() {
                return _interp.get_attr(&def, "delay");
            }
            Ok(Value::none())
        }

        fn request_rate(
            #[positional_only] self_val: PyValueRef<'_>,
            useragent: PyStr<'_>,
        ) -> Result<Value> {
            let mt = call_method(_interp, self_val.0, "mtime", &[])?;
            if !_interp.truthy_value(&mt)? {
                return Ok(Value::none());
            }
            let entries = _interp.get_attr(self_val.0, "entries")?;
            for entry in _interp.collect_iterable(&entries)? {
                let applies = call_method(
                    _interp,
                    &entry,
                    "applies_to",
                    &[Value::string(useragent.0.as_ref())],
                )?;
                if _interp.truthy_value(&applies)? {
                    return _interp.get_attr(&entry, "req_rate");
                }
            }
            let def = _interp.get_attr(self_val.0, "default_entry")?;
            if !def.is_none() {
                return _interp.get_attr(&def, "req_rate");
            }
            Ok(Value::none())
        }

        fn site_maps(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let sitemaps = _interp.get_attr(self_val.0, "sitemaps")?;
            if !_interp.truthy_value(&sitemaps)? {
                return Ok(Value::none());
            }
            Ok(sitemaps)
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let entries = _interp.get_attr(self_val.0, "entries")?;
            let mut items = _interp.collect_iterable(&entries)?;
            let def = _interp.get_attr(self_val.0, "default_entry")?;
            if !def.is_none() {
                items.push(def);
            }
            let mut strs = Vec::with_capacity(items.len());
            for item in items {
                let s = call_pos(_interp, Value::builtin_function("str"), &[item])?;
                strs.push(s.as_str().unwrap_or_default().to_string());
            }
            Ok(Value::string(strs.join("\n\n")))
        }
    }
}
