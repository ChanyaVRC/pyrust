// `xmlrpc.client` module — 100% native-Rust Python 3.12 XML-RPC client and
// wire-format codec using PyRust's expanded macro toolkit (`pyrust_module!`,
// `exception`, `class`, `#[native_state]`, `#[varargs]`, `#[varkwargs]`,
// `PyBytesLike`).
//
// Provides `Error`, `ProtocolError`, `ResponseError`, `Fault`, `DateTime`,
// `Binary`, `Marshaller`, `Unmarshaller`, `ExpatParser`, `getparser`, `dumps`,
// `loads`, `Transport`, `SafeTransport`, `ServerProxy`, `MultiCall`, and
// `MultiCallIterator`.
//
// Reference: <https://docs.python.org/3/library/xmlrpc.client.html>

use std::time::{SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyBytesLike, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr, value_class};
use crate::value::{PyDict, PyKey, Value, ValueKind};

const MAXINT: i64 = 2_147_483_647;
const MININT: i64 = -2_147_483_648;

// ── Shared helpers ───────────────────────────────────────────────────────────

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

fn xc_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("xmlrpc.client")?;
    interp.get_attr(&m, name)
}

fn is_instance_of_xc(interp: &mut Interpreter, val: &Value, cls_name: &str) -> Result<bool> {
    let cls = xc_attr(interp, cls_name)?;
    let res = call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[val.clone(), cls],
    )?;
    interp.truthy_value(&res)
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

fn xml_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch == '&' {
            let rest = &s[i + 1..];
            if let Some(semi) = rest.find(';') {
                let ent = &rest[..semi];
                let decoded: Option<char> = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                        u32::from_str_radix(&ent[2..], 16)
                            .ok()
                            .and_then(char::from_u32)
                    }
                    _ if ent.starts_with('#') => {
                        ent[1..].parse::<u32>().ok().and_then(char::from_u32)
                    }
                    _ => None,
                };
                if let Some(c) = decoded {
                    out.push(c);
                    let target_end = i + 1 + semi;
                    while let Some(&(next_i, _)) = chars.peek() {
                        chars.next();
                        if next_i == target_end {
                            break;
                        }
                    }
                    continue;
                }
            }
        }
        out.push(ch);
    }
    out
}

// ── Pure-Rust RFC 4648 Base64 codec ──────────────────────────────────────────

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode_raw(input: &[u8]) -> String {
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

fn base64_encode_mime(input: &[u8]) -> String {
    if input.is_empty() {
        return String::new();
    }
    let raw = base64_encode_raw(input);
    let mut out = String::with_capacity(raw.len() + raw.len() / 76 + 1);
    for chunk in raw.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    out
}

fn base64_decode(input: &[u8]) -> Result<Vec<u8>> {
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
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    format!("invalid base64 byte: {b}"),
                ));
            }
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

// ── ISO 8601 DateTime helpers ────────────────────────────────────────────────

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

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m_u = m as u64;
    let doy = (153 * (if m_u > 2 { m_u - 3 } else { m_u + 9 }) + 2) / 5 + (d as u64) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + (doe as i64) - 719_468
}

fn unix_to_iso8601(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let secs = ts.rem_euclid(86_400) as u32;
    let (y, m, d) = civil_from_days(days);
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    let ss = secs % 60;
    format!("{y:04}{m:02}{d:02}T{hh:02}:{mm:02}:{ss:02}")
}

fn parse_iso8601_parts(s: &str) -> Option<(i64, i64, i64, i64, i64, i64)> {
    let s = s.trim();
    let (date_part, time_part) = s.split_once('T').or_else(|| s.split_once('t'))?;
    let date_digits: String = date_part.chars().filter(|c| c.is_ascii_digit()).collect();
    if date_digits.len() != 8 {
        return None;
    }
    let y: i64 = date_digits[0..4].parse().ok()?;
    let mo: i64 = date_digits[4..6].parse().ok()?;
    let d: i64 = date_digits[6..8].parse().ok()?;
    // Strip optional trailing Z or timezone offset.
    let time_clean = time_part
        .trim_end_matches(['Z', 'z'])
        .split(['+', '-', '.'])
        .next()
        .unwrap_or(time_part);
    let tparts: Vec<&str> = time_clean.split(':').collect();
    if tparts.len() != 3 {
        return None;
    }
    let hh: i64 = tparts[0].parse().ok()?;
    let mm: i64 = tparts[1].parse().ok()?;
    let ss: i64 = tparts[2].parse().ok()?;
    Some((y, mo, d, hh, mm, ss))
}

fn convert_to_iso8601_str(interp: &mut Interpreter, value: &Value) -> Result<String> {
    if let Some(s) = value.as_str() {
        return Ok(s.trim().to_string());
    }
    if value.is_none() || value.as_int() == Some(0) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        return Ok(unix_to_iso8601(now));
    }
    if let Some(i) = value.as_int() {
        return Ok(unix_to_iso8601(i));
    }
    if let Some(f) = value_as_f64(value) {
        return Ok(unix_to_iso8601(f.floor() as i64));
    }
    if is_instance_of_xc(interp, value, "DateTime")? {
        let v = interp.get_attr(value, "value")?;
        return Ok(v.as_str().unwrap_or_default().to_string());
    }
    // Check datetime.datetime (`year`, `month`, `day`, `hour`, `minute`, `second`)
    if let (Ok(y), Ok(mo), Ok(d), Ok(h), Ok(mi), Ok(s)) = (
        interp.get_attr(value, "year"),
        interp.get_attr(value, "month"),
        interp.get_attr(value, "day"),
        interp.get_attr(value, "hour"),
        interp.get_attr(value, "minute"),
        interp.get_attr(value, "second"),
    ) && let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) = (
        y.as_int(),
        mo.as_int(),
        d.as_int(),
        h.as_int(),
        mi.as_int(),
        s.as_int(),
    ) {
        return Ok(format!("{y:04}{mo:02}{d:02}T{h:02}:{mi:02}:{s:02}"));
    }
    // Tuple / struct_time of >= 6 ints
    if let Ok(items) = interp.collect_iterable(value)
        && items.len() >= 6
        && let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) = (
            items[0].as_int(),
            items[1].as_int(),
            items[2].as_int(),
            items[3].as_int(),
            items[4].as_int(),
            items[5].as_int(),
        )
    {
        return Ok(format!("{y:04}{mo:02}{d:02}T{h:02}:{mi:02}:{s:02}"));
    }
    Err(PyError::named(
        "TypeError",
        "cannot convert argument to xmlrpc.client.DateTime",
    ))
}

fn datetime_cmp_strings(
    interp: &mut Interpreter,
    self_val: &Value,
    other: &Value,
) -> Result<Option<(String, String)>> {
    let lhs = interp
        .get_attr(self_val, "value")?
        .as_str()
        .unwrap_or_default()
        .to_string();
    if is_instance_of_xc(interp, other, "DateTime")? {
        let rhs = interp
            .get_attr(other, "value")?
            .as_str()
            .unwrap_or_default()
            .to_string();
        return Ok(Some((lhs, rhs)));
    }
    if let Some(s) = other.as_str() {
        return Ok(Some((lhs, s.to_string())));
    }
    if let (Ok(y), Ok(mo), Ok(d), Ok(h), Ok(mi), Ok(s)) = (
        interp.get_attr(other, "year"),
        interp.get_attr(other, "month"),
        interp.get_attr(other, "day"),
        interp.get_attr(other, "hour"),
        interp.get_attr(other, "minute"),
        interp.get_attr(other, "second"),
    ) && let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(s)) = (
        y.as_int(),
        mo.as_int(),
        d.as_int(),
        h.as_int(),
        mi.as_int(),
        s.as_int(),
    ) {
        return Ok(Some((
            lhs,
            format!("{y:04}{mo:02}{d:02}T{h:02}:{mi:02}:{s:02}"),
        )));
    }
    Ok(None)
}

// ── Marshaller (Rust serialization engine) ───────────────────────────────────

fn marshal_value(
    interp: &mut Interpreter,
    val: &Value,
    allow_none: bool,
    out: &mut String,
    depth: usize,
) -> Result<()> {
    if depth > 128 {
        return Err(PyError::named(
            "TypeError",
            "cannot marshal recursive structure",
        ));
    }
    if val.is_none() {
        if !allow_none {
            return Err(PyError::named(
                "TypeError",
                "cannot marshal None unless allow_none is enabled",
            ));
        }
        out.push_str("<value><nil/></value>");
        return Ok(());
    }
    match val.kind() {
        ValueKind::Bool(b) => {
            out.push_str("<value><boolean>");
            out.push(if b { '1' } else { '0' });
            out.push_str("</boolean></value>\n");
            return Ok(());
        }
        ValueKind::Int(i) => {
            if !(MININT..=MAXINT).contains(&i) {
                return Err(PyError::named(
                    "OverflowError",
                    "int exceeds XML-RPC limits",
                ));
            }
            out.push_str(&format!("<value><int>{i}</int></value>\n"));
            return Ok(());
        }
        ValueKind::BigInt(_) => {
            return Err(PyError::named(
                "OverflowError",
                "int exceeds XML-RPC limits",
            ));
        }
        ValueKind::Float(_) => {
            let s = render_value_repr(interp, val)?;
            out.push_str(&format!("<value><double>{s}</double></value>\n"));
            return Ok(());
        }
        ValueKind::Str(s) => {
            out.push_str("<value><string>");
            out.push_str(&xml_escape(s));
            out.push_str("</string></value>\n");
            return Ok(());
        }
        ValueKind::Bytes(b) => {
            out.push_str("<value><base64>\n");
            out.push_str(&base64_encode_mime(b));
            out.push_str("</base64></value>\n");
            return Ok(());
        }
        ValueKind::List(rc) => {
            let items: Vec<Value> = rc.clone();
            out.push_str("<value><array><data>\n");
            for item in &items {
                marshal_value(interp, item, allow_none, out, depth + 1)?;
            }
            out.push_str("</data></array></value>\n");
            return Ok(());
        }
        ValueKind::Tuple(items) => {
            out.push_str("<value><array><data>\n");
            for item in items.iter() {
                marshal_value(interp, item, allow_none, out, depth + 1)?;
            }
            out.push_str("</data></array></value>\n");
            return Ok(());
        }
        ValueKind::Dict(_) => {
            let entries: Vec<(String, Value)> = {
                let d = val.as_dict().unwrap();
                let mut pairs = Vec::with_capacity(d.len());
                for (k, v) in d.iter() {
                    let PyKey::Str(sv) = k else {
                        return Err(PyError::named("TypeError", "dictionary key must be string"));
                    };
                    let ks = sv.as_str().ok_or_else(|| {
                        PyError::named("TypeError", "dictionary key must be string")
                    })?;
                    pairs.push((ks.to_string(), v.clone()));
                }
                pairs
            };
            out.push_str("<value><struct>\n");
            for (k, v) in entries {
                out.push_str("<member>\n<name>");
                out.push_str(&xml_escape(&k));
                out.push_str("</name>\n");
                marshal_value(interp, &v, allow_none, out, depth + 1)?;
                out.push_str("</member>\n");
            }
            out.push_str("</struct></value>\n");
            return Ok(());
        }
        _ => {}
    }

    // Handle bytearray
    if let Some(ba) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
        out.push_str("<value><base64>\n");
        out.push_str(&base64_encode_mime(&ba));
        out.push_str("</base64></value>\n");
        return Ok(());
    }

    // Handle xmlrpc.client.DateTime
    if is_instance_of_xc(interp, val, "DateTime")? {
        let v = interp
            .get_attr(val, "value")?
            .as_str()
            .unwrap_or_default()
            .to_string();
        out.push_str("<value><dateTime.iso8601>");
        out.push_str(&xml_escape(&v));
        out.push_str("</dateTime.iso8601></value>\n");
        return Ok(());
    }

    // Handle xmlrpc.client.Binary
    if is_instance_of_xc(interp, val, "Binary")? {
        let data = interp.get_attr(val, "data")?;
        let bytes = match data.kind() {
            ValueKind::Bytes(b) => b.as_ref().clone(),
            _ => Vec::new(),
        };
        out.push_str("<value><base64>\n");
        out.push_str(&base64_encode_mime(&bytes));
        out.push_str("</base64></value>\n");
        return Ok(());
    }

    // Handle datetime.datetime
    if let Ok(dt_mod) = interp.load_module("datetime")
        && let Ok(dt_cls) = interp.get_attr(&dt_mod, "datetime")
    {
        let is_dt = call_pos(
            interp,
            Value::builtin_function("isinstance"),
            &[val.clone(), dt_cls],
        )?;
        if interp.truthy_value(&is_dt)? {
            let iso = convert_to_iso8601_str(interp, val)?;
            out.push_str("<value><dateTime.iso8601>");
            out.push_str(&iso);
            out.push_str("</dateTime.iso8601></value>\n");
            return Ok(());
        }
    }

    // Fallback: arbitrary instance with `__dict__` serialized as `<struct>`
    if let Ok(dict_val) = interp.get_attr(val, "__dict__")
        && dict_val.as_dict().is_some()
    {
        return marshal_value(interp, &dict_val, allow_none, out, depth + 1);
    }

    Err(PyError::named(
        "TypeError",
        format!(
            "cannot marshal {} objects",
            crate::interpreter::value_type_name_str(val)
        ),
    ))
}

fn marshal_dumps(interp: &mut Interpreter, values: &Value, allow_none: bool) -> Result<String> {
    let mut out = String::new();
    if is_instance_of_xc(interp, values, "Fault")? {
        let fc = interp.get_attr(values, "faultCode")?;
        let fs = interp.get_attr(values, "faultString")?;
        let mut d = PyDict::default();
        d.insert(PyKey::str_from("faultCode"), fc);
        d.insert(PyKey::str_from("faultString"), fs);
        out.push_str("<fault>\n");
        marshal_value(interp, &Value::dict(d), allow_none, &mut out, 0)?;
        out.push_str("</fault>\n");
        return Ok(out);
    }
    let items = interp.collect_iterable(values)?;
    out.push_str("<params>\n");
    for item in &items {
        out.push_str("<param>\n");
        marshal_value(interp, item, allow_none, &mut out, 0)?;
        out.push_str("</param>\n");
    }
    out.push_str("</params>\n");
    Ok(out)
}

// ── Pure-Rust XML 1.0 Streaming Parser (ExpatParser) ─────────────────────────

struct ExpatParserState {
    target: Value,
    buffer: String,
}

fn parse_xml_attrs(attr_src: &str) -> Value {
    let mut dict = PyDict::default();
    let bytes = attr_src.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= len {
            break;
        }
        let k_start = i;
        while i < len && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        let key = &attr_src[k_start..i];
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < len && bytes[i] == b'=' {
            i += 1;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < len && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let v_start = i;
                while i < len && bytes[i] != quote {
                    i += 1;
                }
                let val = xml_unescape(&attr_src[v_start..i]);
                if i < len {
                    i += 1;
                }
                if !key.is_empty() {
                    dict.insert(PyKey::str_from(key), Value::string(val));
                }
            }
        }
    }
    Value::dict(dict)
}

fn parse_xml_document(interp: &mut Interpreter, target: &Value, xml: &str) -> Result<()> {
    let mut pos = 0usize;
    let bytes = xml.as_bytes();
    let len = bytes.len();

    while pos < len {
        if let Some(lt_rel) = xml[pos..].find('<') {
            if lt_rel > 0 {
                let text = xml_unescape(&xml[pos..pos + lt_rel]);
                call_method(interp, target, "data", &[Value::string(text)])?;
            }
            pos += lt_rel;
            // Comment <!-- ... -->
            if xml[pos..].starts_with("<!--") {
                let end = xml[pos + 4..]
                    .find("-->")
                    .ok_or_else(|| PyError::named("ValueError", "unclosed XML comment"))?;
                pos = pos + 4 + end + 3;
                continue;
            }
            // CDATA <![CDATA[ ... ]]>
            if xml[pos..].starts_with("<![CDATA[") {
                let end = xml[pos + 9..]
                    .find("]]>")
                    .ok_or_else(|| PyError::named("ValueError", "unclosed XML CDATA"))?;
                let cdata = &xml[pos + 9..pos + 9 + end];
                call_method(interp, target, "data", &[Value::string(cdata)])?;
                pos = pos + 9 + end + 3;
                continue;
            }
            // Processing instruction / XML declaration <? ... ?>
            if xml[pos..].starts_with("<?") {
                let end = xml[pos + 2..]
                    .find("?>")
                    .ok_or_else(|| PyError::named("ValueError", "unclosed XML declaration"))?;
                pos = pos + 2 + end + 2;
                continue;
            }
            // DOCTYPE <!DOCTYPE ... >
            if xml[pos..].starts_with("<!") {
                let end = xml[pos + 2..]
                    .find('>')
                    .ok_or_else(|| PyError::named("ValueError", "unclosed XML directive"))?;
                pos = pos + 2 + end + 1;
                continue;
            }
            let gt_rel = xml[pos + 1..]
                .find('>')
                .ok_or_else(|| PyError::named("ValueError", "unclosed XML tag"))?;
            let inside = xml[pos + 1..pos + 1 + gt_rel].trim();
            pos = pos + 1 + gt_rel + 1;

            if let Some(close_name) = inside.strip_prefix('/') {
                let tag = close_name.trim();
                call_method(interp, target, "end", &[Value::string(tag)])?;
            } else {
                let (body, self_closing) = match inside.strip_suffix('/') {
                    Some(b) => (b.trim(), true),
                    None => (inside, false),
                };
                let (tag, attrs_src) = match body.find(|c: char| c.is_ascii_whitespace()) {
                    Some(idx) => (&body[..idx], &body[idx..]),
                    None => (body, ""),
                };
                let attrs = parse_xml_attrs(attrs_src);
                call_method(interp, target, "start", &[Value::string(tag), attrs])?;
                if self_closing {
                    call_method(interp, target, "end", &[Value::string(tag)])?;
                }
            }
        } else {
            let text = xml_unescape(&xml[pos..]);
            if !text.is_empty() {
                call_method(interp, target, "data", &[Value::string(text)])?;
            }
            break;
        }
    }
    Ok(())
}

// ── Unmarshaller state ───────────────────────────────────────────────────────

struct UnmarshallerState {
    resp_type: Option<String>,
    stack: Vec<Value>,
    marks: Vec<usize>,
    data: Vec<String>,
    value: bool,
    methodname: Option<String>,
    use_datetime: bool,
    use_builtin_types: bool,
}

impl UnmarshallerState {
    fn new(use_datetime: bool, use_builtin_types: bool) -> Self {
        Self {
            resp_type: None,
            stack: Vec::new(),
            marks: Vec::new(),
            data: Vec::new(),
            value: false,
            methodname: None,
            use_datetime,
            use_builtin_types,
        }
    }
}

// ── Client proxy / multicall state ───────────────────────────────────────────

struct MethodState {
    send: Value,
    name: String,
}

struct MultiCallMethodState {
    call_list: Value,
    name: String,
}

fn parse_xmlrpc_uri(uri: &str) -> Result<(String, String, String)> {
    let (scheme, rest) = if let Some(r) = uri.strip_prefix("http://") {
        ("http".to_string(), r)
    } else if let Some(r) = uri.strip_prefix("https://") {
        ("https".to_string(), r)
    } else {
        return Err(PyError::named("OSError", "unsupported XML-RPC protocol"));
    };
    let (host, handler) = match rest.find('/') {
        Some(idx) => (rest[..idx].to_string(), rest[idx..].to_string()),
        None => (rest.to_string(), "/RPC2".to_string()),
    };
    let handler = if handler.is_empty() {
        "/RPC2".to_string()
    } else {
        handler
    };
    Ok((scheme, host, handler))
}

pyrust_module! {
    constants {
        "PARSE_ERROR" => Value::int(-32700),
        "SERVER_ERROR" => Value::int(-32600),
        "APPLICATION_ERROR" => Value::int(-32500),
        "SYSTEM_ERROR" => Value::int(-32400),
        "TRANSPORT_ERROR" => Value::int(-32300),
        "NOT_WELLFORMED_ERROR" => Value::int(-32700),
        "UNSUPPORTED_ENCODING" => Value::int(-32701),
        "INVALID_ENCODING_CHAR" => Value::int(-32702),
        "INVALID_XMLRPC" => Value::int(-32600),
        "METHOD_NOT_FOUND" => Value::int(-32601),
        "INVALID_METHOD_PARAMS" => Value::int(-32602),
        "INTERNAL_ERROR" => Value::int(-32603),
        "MAXINT" => Value::int(MAXINT),
        "MININT" => Value::int(MININT),
        "__version__" => Value::string("3.12"),
    }

    // ── Exceptions ───────────────────────────────────────────────────────────

    exception Error;
    exception ResponseError(Error);

    class ProtocolError("Error") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            url: PyValue,
            errcode: PyValue,
            errmsg: PyValue,
            headers: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "url", url.0)?;
            _interp.assign_attr(self_val.0.clone(), "errcode", errcode.0)?;
            _interp.assign_attr(self_val.0.clone(), "errmsg", errmsg.0)?;
            _interp.assign_attr(self_val.0.clone(), "headers", headers.0)?;
            _interp.assign_attr(self_val.0.clone(), "args", Value::tuple(Vec::new()))?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = value_class(self_val.0);
            let cname = _interp
                .get_attr(&cls, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "ProtocolError".to_string());
            let url = _interp.get_attr(self_val.0, "url")?;
            let code = _interp.get_attr(self_val.0, "errcode")?;
            let msg = _interp.get_attr(self_val.0, "errmsg")?;
            let url_s = call_pos(_interp, Value::builtin_function("str"), &[url])?;
            let code_s = call_pos(_interp, Value::builtin_function("str"), &[code])?;
            let msg_s = call_pos(_interp, Value::builtin_function("str"), &[msg])?;
            Ok(Value::string(format!(
                "<{cname} for {}: {} {}>",
                url_s.as_str().unwrap_or_default(),
                code_s.as_str().unwrap_or_default(),
                msg_s.as_str().unwrap_or_default()
            )))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "__repr__", &[])
        }
    }

    class Fault("Error") {
        #[allow(non_snake_case)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            faultCode: PyValue,
            faultString: PyValue,
            #[varkwargs] _extra: IndexMap<String, Value>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "faultCode", faultCode.0)?;
            _interp.assign_attr(self_val.0.clone(), "faultString", faultString.0)?;
            _interp.assign_attr(self_val.0.clone(), "args", Value::tuple(Vec::new()))?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = value_class(self_val.0);
            let cname = _interp
                .get_attr(&cls, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "Fault".to_string());
            let code = _interp.get_attr(self_val.0, "faultCode")?;
            let msg = _interp.get_attr(self_val.0, "faultString")?;
            let code_s = call_pos(_interp, Value::builtin_function("str"), &[code])?;
            let msg_r = render_value_repr(_interp, &msg)?;
            Ok(Value::string(format!(
                "<{cname} {}: {msg_r}>",
                code_s.as_str().unwrap_or_default()
            )))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "__repr__", &[])
        }
    }

    // ── DateTime & Binary ────────────────────────────────────────────────────

    class DateTime {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] value: OptValueRef<'_>,
        ) -> Result<Value> {
            let val = value
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::int(0));
            let iso = convert_to_iso8601_str(_interp, &val)?;
            _interp.assign_attr(self_val.0.clone(), "value", Value::string(iso))?;
            Ok(Value::none())
        }

        fn __lt__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            match datetime_cmp_strings(_interp, self_val.0, &other.0)? {
                Some((a, b)) => Ok(Value::bool_(a < b)),
                None => Ok(Value::not_implemented()),
            }
        }

        fn __le__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            match datetime_cmp_strings(_interp, self_val.0, &other.0)? {
                Some((a, b)) => Ok(Value::bool_(a <= b)),
                None => Ok(Value::not_implemented()),
            }
        }

        fn __gt__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            match datetime_cmp_strings(_interp, self_val.0, &other.0)? {
                Some((a, b)) => Ok(Value::bool_(a > b)),
                None => Ok(Value::not_implemented()),
            }
        }

        fn __ge__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            match datetime_cmp_strings(_interp, self_val.0, &other.0)? {
                Some((a, b)) => Ok(Value::bool_(a >= b)),
                None => Ok(Value::not_implemented()),
            }
        }

        fn __eq__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            match datetime_cmp_strings(_interp, self_val.0, &other.0)? {
                Some((a, b)) => Ok(Value::bool_(a == b)),
                None => Ok(Value::not_implemented()),
            }
        }

        fn timetuple(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let val = _interp.get_attr(self_val.0, "value")?;
            let s = val.as_str().unwrap_or_default().to_string();
            let (y, mo, d, hh, mm, ss) = parse_iso8601_parts(&s).ok_or_else(|| {
                PyError::named(
                    "ValueError",
                    format!("time data {s:?} does not match format '%Y%m%dT%H:%M:%S'"),
                )
            })?;
            let days = days_from_civil(y, mo as u32, d as u32);
            let jan1 = days_from_civil(y, 1, 1);
            let wday = (days + 3).rem_euclid(7);
            let yday = (days - jan1) + 1;
            let time_mod = _interp.load_module("time")?;
            let st_cls = _interp.get_attr(&time_mod, "struct_time")?;
            let tup = Value::tuple(vec![
                Value::int(y),
                Value::int(mo),
                Value::int(d),
                Value::int(hh),
                Value::int(mm),
                Value::int(ss),
                Value::int(wday),
                Value::int(yday),
                Value::int(-1),
            ]);
            call_pos(_interp, st_cls, &[tup])
        }

        fn decode(#[positional_only] self_val: PyValueRef<'_>, data: PyValue) -> Result<Value> {
            let s = call_pos(_interp, Value::builtin_function("str"), &[data.0])?;
            let trimmed = s.as_str().unwrap_or_default().trim().to_string();
            _interp.assign_attr(self_val.0.clone(), "value", Value::string(trimmed))?;
            Ok(Value::none())
        }

        fn encode(#[positional_only] self_val: PyValueRef<'_>, out: PyValue) -> Result<Value> {
            let val = _interp
                .get_attr(self_val.0, "value")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let xml = format!("<value><dateTime.iso8601>{val}</dateTime.iso8601></value>\n");
            call_method(_interp, &out.0, "write", &[Value::string(xml)])?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "value")
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = value_class(self_val.0);
            let cname = _interp
                .get_attr(&cls, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "DateTime".to_string());
            let val = _interp.get_attr(self_val.0, "value")?;
            let r = render_value_repr(_interp, &val)?;
            Ok(Value::string(format!("<{cname} {r}>")))
        }
    }

    class Binary {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] data: OptValueRef<'_>,
        ) -> Result<Value> {
            let bytes_val = match data.as_value() {
                None => Value::bytes(Vec::new()),
                Some(v) if v.is_none() => Value::bytes(Vec::new()),
                Some(v) => match v.kind() {
                    ValueKind::Bytes(_) => v.clone(),
                    _ => {
                        if let Some(ba) = pyrust_builtins::bytearray::as_bytearray_snapshot(v) {
                            Value::bytes(ba)
                        } else {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "expected bytes or bytearray, not {}",
                                    crate::interpreter::value_type_name_str(v)
                                ),
                            ));
                        }
                    }
                },
            };
            _interp.assign_attr(self_val.0.clone(), "data", bytes_val)?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let data = _interp.get_attr(self_val.0, "data")?;
            if data.is_none() {
                return Ok(Value::string(""));
            }
            call_pos(_interp, Value::builtin_function("str"), &[data])
        }

        fn __eq__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let my_data = _interp.get_attr(self_val.0, "data")?;
            let other_data = if is_instance_of_xc(_interp, &other.0, "Binary")? {
                _interp.get_attr(&other.0, "data")?
            } else {
                other.0
            };
            _interp.eval_binary(my_data, crate::ast::BinaryOp::Eq, other_data)
        }

        fn decode(#[positional_only] self_val: PyValueRef<'_>, data: PyValue) -> Result<Value> {
            let raw_bytes: Vec<u8> = match data.0.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                ValueKind::Str(s) => s.as_bytes().to_vec(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "Binary.decode() expects bytes or str",
                    ));
                }
            };
            let decoded = base64_decode(&raw_bytes)?;
            _interp.assign_attr(self_val.0.clone(), "data", Value::bytes(decoded))?;
            Ok(Value::none())
        }

        fn encode(#[positional_only] self_val: PyValueRef<'_>, out: PyValue) -> Result<Value> {
            let data = _interp.get_attr(self_val.0, "data")?;
            let bytes = match data.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                _ => Vec::new(),
            };
            let xml = format!("<value><base64>\n{}</base64></value>\n", base64_encode_mime(&bytes));
            call_method(_interp, &out.0, "write", &[Value::string(xml)])?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = value_class(self_val.0);
            let cname = _interp
                .get_attr(&cls, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "Binary".to_string());
            Ok(Value::string(format!("<{cname} object>")))
        }
    }

    // ── Marshaller / Unmarshaller / ExpatParser ──────────────────────────────

    class Marshaller {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(false.into())] allow_none: PyBool,
        ) -> Result<Value> {
            let enc = match encoding.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => Value::string("utf-8"),
            };
            _interp.assign_attr(self_val.0.clone(), "encoding", enc)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "allow_none",
                Value::bool_(allow_none.0),
            )?;
            Ok(Value::none())
        }

        fn dumps(#[positional_only] self_val: PyValueRef<'_>, values: PyValue) -> Result<Value> {
            let allow_none = get_attr_truthy(_interp, self_val.0, "allow_none")?;
            let xml = marshal_dumps(_interp, &values.0, allow_none)?;
            Ok(Value::string(xml))
        }
    }

    #[native_state(UnmarshallerState)]
    class Unmarshaller {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(false.into())] use_datetime: PyBool,
            #[default(false.into())] use_builtin_types: PyBool,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                UnmarshallerState::new(use_datetime.0, use_builtin_types.0),
            )?;
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (resp_type, stack) = with_state(self_val.0, |st| {
                Ok((st.resp_type.clone(), st.stack.clone()))
            })?;
            if resp_type.is_none() || !with_state(self_val.0, |st| Ok(st.marks.is_empty()))? {
                let re_cls = xc_attr(_interp, "ResponseError")?;
                let exc = call_pos(_interp, re_cls, &[])?;
                return Err(PyError::raised(exc));
            }
            if resp_type.as_deref() == Some("fault") {
                let fault_cls = xc_attr(_interp, "Fault")?;
                let mut fc = Value::int(0);
                let mut fs = Value::string("");
                if let Some(d) = stack.first().and_then(|v| v.as_dict()) {
                    if let Some(c) = d.get(&PyKey::str_from("faultCode")) {
                        fc = c.clone();
                    }
                    if let Some(s) = d.get(&PyKey::str_from("faultString")) {
                        fs = s.clone();
                    }
                }
                let exc = call_pos(_interp, fault_cls, &[fc, fs])?;
                return Err(PyError::raised(exc));
            }
            Ok(Value::tuple(stack))
        }

        fn getmethodname(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state(self_val.0, |st| match &st.methodname {
                Some(name) => Ok(Value::string(name.clone())),
                None => Ok(Value::none()),
            })
        }

        fn start(
            #[positional_only] self_val: PyValueRef<'_>,
            tag: PyStr<'_>,
            _attrs: PyValue,
        ) -> Result<Value> {
            let short = tag.0.rsplit(':').next().unwrap_or(&tag.0);
            with_state_mut(self_val.0, |st| {
                if short == "array" || short == "struct" {
                    let mark = st.stack.len();
                    st.marks.push(mark);
                }
                st.data.clear();
                st.value = short == "value";
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn data(#[positional_only] self_val: PyValueRef<'_>, text: PyStr<'_>) -> Result<Value> {
            with_state_mut(self_val.0, |st| {
                st.data.push(text.0.to_string());
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn end(#[positional_only] self_val: PyValueRef<'_>, tag: PyStr<'_>) -> Result<Value> {
            let short = tag.0.rsplit(':').next().unwrap_or(&tag.0);
            let (raw_data, value_flag, use_dt, use_builtin) = with_state_mut(self_val.0, |st| {
                let joined = st.data.concat();
                let vf = st.value;
                st.data.clear();
                Ok((joined, vf, st.use_datetime, st.use_builtin_types))
            })?;

            match short {
                "nil" => {
                    with_state_mut(self_val.0, |st| {
                        st.stack.push(Value::none());
                        st.value = false;
                        Ok(())
                    })?;
                }
                "boolean" => {
                    let b = match raw_data.trim() {
                        "0" => false,
                        "1" => true,
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                "bad boolean value",
                            ));
                        }
                    };
                    with_state_mut(self_val.0, |st| {
                        st.stack.push(Value::bool_(b));
                        st.value = false;
                        Ok(())
                    })?;
                }
                "i1" | "i2" | "i4" | "i8" | "int" | "biginteger" => {
                    let trimmed = raw_data.trim();
                    let val = if let Ok(i) = trimmed.parse::<i64>() {
                        Value::int(i)
                    } else if let Ok(bi) = trimmed.parse::<num_bigint::BigInt>() {
                        Value::bigint(bi)
                    } else {
                        return Err(PyError::named(
                            "ValueError",
                            format!("invalid integer literal: {trimmed:?}"),
                        ));
                    };
                    with_state_mut(self_val.0, |st| {
                        st.stack.push(val);
                        st.value = false;
                        Ok(())
                    })?;
                }
                "double" | "float" => {
                    let f: f64 = raw_data.trim().parse().map_err(|_| {
                        PyError::named("ValueError", format!("invalid float: {raw_data:?}"))
                    })?;
                    with_state_mut(self_val.0, |st| {
                        st.stack.push(Value::float(f));
                        st.value = false;
                        Ok(())
                    })?;
                }
                "string" | "name" => {
                    with_state_mut(self_val.0, |st| {
                        st.stack.push(Value::string(raw_data));
                        st.value = false;
                        Ok(())
                    })?;
                }
                "value" => {
                    if value_flag {
                        with_state_mut(self_val.0, |st| {
                            st.stack.push(Value::string(raw_data));
                            st.value = false;
                            Ok(())
                        })?;
                    }
                }
                "array" => {
                    with_state_mut(self_val.0, |st| {
                        let mark = st.marks.pop().unwrap_or(0);
                        let items: Vec<Value> = st.stack.drain(mark..).collect();
                        st.stack.push(Value::list(items));
                        st.value = false;
                        Ok(())
                    })?;
                }
                "struct" => {
                    with_state_mut(self_val.0, |st| {
                        let mark = st.marks.pop().unwrap_or(0);
                        let items: Vec<Value> = st.stack.drain(mark..).collect();
                        let mut d = PyDict::default();
                        let mut iter = items.into_iter();
                        while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                            let ks = k.as_str().unwrap_or_default().to_string();
                            d.insert(PyKey::str_from(&ks), v);
                        }
                        st.stack.push(Value::dict(d));
                        st.value = false;
                        Ok(())
                    })?;
                }
                "base64" => {
                    let decoded = base64_decode(raw_data.as_bytes())?;
                    let val = if use_builtin {
                        Value::bytes(decoded)
                    } else {
                        let bin_cls = xc_attr(_interp, "Binary")?;
                        call_pos(_interp, bin_cls, &[Value::bytes(decoded)])?
                    };
                    with_state_mut(self_val.0, |st| {
                        st.stack.push(val);
                        st.value = false;
                        Ok(())
                    })?;
                }
                "dateTime.iso8601" => {
                    let trimmed = raw_data.trim().to_string();
                    let val = if use_dt || use_builtin {
                        let (y, mo, d, hh, mm, ss) = parse_iso8601_parts(&trimmed)
                            .ok_or_else(|| {
                                PyError::named(
                                    "ValueError",
                                    format!("invalid ISO8601 datetime: {trimmed:?}"),
                                )
                            })?;
                        let dt_mod = _interp.load_module("datetime")?;
                        let dt_cls = _interp.get_attr(&dt_mod, "datetime")?;
                        call_pos(
                            _interp,
                            dt_cls,
                            &[
                                Value::int(y),
                                Value::int(mo),
                                Value::int(d),
                                Value::int(hh),
                                Value::int(mm),
                                Value::int(ss),
                            ],
                        )?
                    } else {
                        let dt_cls = xc_attr(_interp, "DateTime")?;
                        call_pos(_interp, dt_cls, &[Value::string(trimmed)])?
                    };
                    with_state_mut(self_val.0, |st| {
                        st.stack.push(val);
                        st.value = false;
                        Ok(())
                    })?;
                }
                "params" => {
                    with_state_mut(self_val.0, |st| {
                        st.resp_type = Some("params".to_string());
                        Ok(())
                    })?;
                }
                "fault" => {
                    with_state_mut(self_val.0, |st| {
                        st.resp_type = Some("fault".to_string());
                        Ok(())
                    })?;
                }
                "methodName" => {
                    with_state_mut(self_val.0, |st| {
                        st.methodname = Some(raw_data);
                        st.resp_type = Some("methodName".to_string());
                        Ok(())
                    })?;
                }
                _ => {}
            }
            Ok(Value::none())
        }
    }

    #[native_state(ExpatParserState)]
    class ExpatParser {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            target: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                ExpatParserState {
                    target: target.0,
                    buffer: String::new(),
                },
            )?;
            Ok(Value::none())
        }

        fn feed(#[positional_only] self_val: PyValueRef<'_>, data: PyValue) -> Result<Value> {
            let chunk = match data.0.kind() {
                ValueKind::Str(s) => s.to_string(),
                ValueKind::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "ExpatParser.feed() expects str or bytes",
                    ));
                }
            };
            with_state_mut(self_val.0, |st| {
                st.buffer.push_str(&chunk);
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (target, xml) = with_state_mut(self_val.0, |st| {
                Ok((st.target.clone(), std::mem::take(&mut st.buffer)))
            })?;
            parse_xml_document(_interp, &target, &xml)?;
            Ok(Value::none())
        }
    }

    fn getparser(
        #[default(false.into())] use_datetime: PyBool,
        #[default(false.into())] use_builtin_types: PyBool,
    ) -> Result<Value> {
        let u_cls = xc_attr(_interp, "Unmarshaller")?;
        let target = call_pos(
            _interp,
            u_cls,
            &[Value::bool_(use_datetime.0), Value::bool_(use_builtin_types.0)],
        )?;
        let p_cls = xc_attr(_interp, "ExpatParser")?;
        let parser = call_pos(_interp, p_cls, std::slice::from_ref(&target))?;
        Ok(Value::tuple(vec![parser, target]))
    }

    fn dumps(
        params: PyValue,
        #[default(OptionalArg::Omitted)] methodname: OptValueRef<'_>,
        #[default(false.into())] methodresponse: PyBool,
        #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
        #[default(false.into())] allow_none: PyBool,
    ) -> Result<Value> {
        let is_fault = is_instance_of_xc(_interp, &params.0, "Fault")?;
        if !is_fault && !matches!(params.0.kind(), ValueKind::Tuple(_)) {
            return Err(PyError::named(
                "AssertionError",
                "argument must be tuple or Fault instance",
            ));
        }
        let enc = encoding
            .as_value()
            .and_then(|v| v.as_str().map(str::to_string));
        let data = marshal_dumps(_interp, &params.0, allow_none.0)?;
        let xml_header = match enc.as_deref() {
            Some(e) if !e.eq_ignore_ascii_case("utf-8") => {
                format!("<?xml version='1.0' encoding='{e}'?>\n")
            }
            _ => "<?xml version='1.0'?>\n".to_string(),
        };
        let out = if let Some(mval) = methodname.as_value()
            && !mval.is_none()
        {
            let mname = mval.as_str().unwrap_or_default();
            format!(
                "{xml_header}<methodCall>\n<methodName>{}</methodName>\n{data}</methodCall>\n",
                xml_escape(mname)
            )
        } else if methodresponse.0 || is_fault {
            format!("{xml_header}<methodResponse>\n{data}</methodResponse>\n")
        } else {
            data
        };
        Ok(Value::string(out))
    }

    fn loads(
        data: PyValue,
        #[default(false.into())] use_datetime: PyBool,
        #[default(false.into())] use_builtin_types: PyBool,
    ) -> Result<Value> {
        let gp = xc_attr(_interp, "getparser")?;
        let pair = call_pos(
            _interp,
            gp,
            &[Value::bool_(use_datetime.0), Value::bool_(use_builtin_types.0)],
        )?;
        let items = _interp.collect_iterable(&pair)?;
        let p = &items[0];
        let u = &items[1];
        call_method(_interp, p, "feed", &[data.0])?;
        call_method(_interp, p, "close", &[])?;
        let res = call_method(_interp, u, "close", &[])?;
        let mname = call_method(_interp, u, "getmethodname", &[])?;
        Ok(Value::tuple(vec![res, mname]))
    }

    // ── Transport, SafeTransport, ServerProxy, MultiCall ─────────────────────

    class Transport {
        constants {
            "user_agent" => Value::string("Python-xmlrpc/3.12"),
            "accept_gzip_encoding" => Value::bool_(true),
            "encode_threshold" => Value::none(),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(false.into())] use_datetime: PyBool,
            #[default(false.into())] use_builtin_types: PyBool,
            #[default(OptionalArg::Omitted)] headers: OptValueRef<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "_use_datetime",
                Value::bool_(use_datetime.0),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_use_builtin_types",
                Value::bool_(use_builtin_types.0),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_connection",
                Value::tuple(vec![Value::none(), Value::none()]),
            )?;
            let hdrs = match headers.as_value() {
                Some(v) if !v.is_none() => Value::list(_interp.collect_iterable(v)?),
                _ => Value::list(Vec::new()),
            };
            _interp.assign_attr(self_val.0.clone(), "_headers", hdrs)?;
            _interp.assign_attr(self_val.0.clone(), "_extra_headers", Value::list(Vec::new()))?;
            Ok(Value::none())
        }

        fn get_host_info(
            #[positional_only] _self: PyValueRef<'_>,
            host: PyValue,
        ) -> Result<Value> {
            let mut extra_headers: Vec<Value> = Vec::new();
            let host_str = if matches!(host.0.kind(), ValueKind::Tuple(_)) {
                let parts = _interp.collect_iterable(&host.0)?;
                let h = parts
                    .first()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                if let Some(d) = parts.get(1).and_then(|v| v.as_dict()) {
                    for (k, v) in d.iter() {
                        if let PyKey::Str(sv) = k
                            && let Some(ks) = sv.as_str()
                        {
                            extra_headers.push(Value::tuple(vec![Value::string(ks), v.clone()]));
                        }
                    }
                }
                h
            } else {
                host.0.as_str().unwrap_or_default().to_string()
            };
            let clean_host = if let Some((auth, h)) = host_str.rsplit_once('@') {
                let b64 = base64_encode_raw(auth.as_bytes());
                extra_headers.push(Value::tuple(vec![
                    Value::string("Authorization"),
                    Value::string(format!("Basic {b64}")),
                ]));
                h.to_string()
            } else {
                host_str
            };
            Ok(Value::tuple(vec![
                Value::string(clean_host),
                Value::list(extra_headers),
                Value::dict(PyDict::default()),
            ]))
        }

        fn make_connection(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyValue,
        ) -> Result<Value> {
            let info = call_method(_interp, self_val.0, "get_host_info", &[host.0])?;
            let parts = _interp.collect_iterable(&info)?;
            let chost = parts[0].clone();
            let extra_headers = parts[1].clone();
            _interp.assign_attr(self_val.0.clone(), "_extra_headers", extra_headers)?;
            let hc = _interp.load_module("http.client")?;
            let conn_cls = _interp.get_attr(&hc, "HTTPConnection")?;
            let conn = call_pos(_interp, conn_cls, std::slice::from_ref(&chost))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_connection",
                Value::tuple(vec![chost, conn.clone()]),
            )?;
            Ok(conn)
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let conn_pair = _interp.get_attr(self_val.0, "_connection")?;
            let parts = _interp.collect_iterable(&conn_pair)?;
            if parts.len() >= 2 && !parts[1].is_none() {
                let _ = call_method(_interp, &parts[1], "close", &[]);
            }
            _interp.assign_attr(
                self_val.0.clone(),
                "_connection",
                Value::tuple(vec![Value::none(), Value::none()]),
            )?;
            Ok(Value::none())
        }

        fn getparser(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let use_dt = _interp.get_attr(self_val.0, "_use_datetime")?;
            let use_bt = _interp.get_attr(self_val.0, "_use_builtin_types")?;
            let gp = xc_attr(_interp, "getparser")?;
            call_pos(_interp, gp, &[use_dt, use_bt])
        }

        fn parse_response(
            #[positional_only] self_val: PyValueRef<'_>,
            response: PyValue,
        ) -> Result<Value> {
            let pair = call_method(_interp, self_val.0, "getparser", &[])?;
            let items = _interp.collect_iterable(&pair)?;
            let p = &items[0];
            let u = &items[1];
            let mut body = call_method(_interp, &response.0, "read", &[])?;
            if let Ok(gh) = _interp.get_attr(&response.0, "getheader")
                && let Ok(enc) = call_pos(_interp, gh, &[Value::string("Content-Encoding"), Value::string("")])
                && enc.as_str().is_some_and(|e| e.eq_ignore_ascii_case("gzip"))
            {
                let gzip_mod = _interp.load_module("gzip")?;
                let decompress = _interp.get_attr(&gzip_mod, "decompress")?;
                body = call_pos(_interp, decompress, &[body])?;
            }
            call_method(_interp, p, "feed", &[body])?;
            call_method(_interp, p, "close", &[])?;
            call_method(_interp, u, "close", &[])
        }

        fn request(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyValue,
            handler: PyStr<'_>,
            request_body: PyBytesLike<'_>,
            #[default(false.into())] verbose: PyBool,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "single_request",
                &[
                    host.0,
                    Value::string(handler.0.as_ref()),
                    Value::bytes(request_body.as_bytes().to_vec()),
                    Value::bool_(verbose.0),
                ],
            )
        }

        fn single_request(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyValue,
            handler: PyStr<'_>,
            request_body: PyBytesLike<'_>,
            #[default(false.into())] _verbose: PyBool,
        ) -> Result<Value> {
            let conn = call_method(
                _interp,
                self_val.0,
                "make_connection",
                std::slice::from_ref(&host.0),
            )?;
            let body_bytes = request_body.as_bytes().to_vec();
            let mut hdrs_dict = PyDict::default();
            let ua = _interp
                .get_attr(self_val.0, "user_agent")?
                .as_str()
                .unwrap_or("Python-xmlrpc/3.12")
                .to_string();
            hdrs_dict.insert(PyKey::str_from("User-Agent"), Value::string(ua));
            hdrs_dict.insert(PyKey::str_from("Content-Type"), Value::string("text/xml"));
            hdrs_dict.insert(
                PyKey::str_from("Content-Length"),
                Value::string(body_bytes.len().to_string()),
            );
            for attr_name in ["_extra_headers", "_headers"] {
                if let Ok(lst) = _interp.get_attr(self_val.0, attr_name) {
                    for entry in _interp.collect_iterable(&lst)? {
                        let pair = _interp.collect_iterable(&entry)?;
                        if pair.len() == 2
                            && let Some(k) = pair[0].as_str()
                        {
                            hdrs_dict.insert(PyKey::str_from(k), pair[1].clone());
                        }
                    }
                }
            }
            call_method(
                _interp,
                &conn,
                "request",
                &[
                    Value::string("POST"),
                    Value::string(handler.0.as_ref()),
                    Value::bytes(body_bytes),
                    Value::dict(hdrs_dict),
                ],
            )?;
            let resp = call_method(_interp, &conn, "getresponse", &[])?;
            let status = _interp.get_attr(&resp, "status")?.as_int().unwrap_or(0);
            if status == 200 {
                let parsed = call_method(_interp, self_val.0, "parse_response", &[resp]);
                let _ = call_method(_interp, self_val.0, "close", &[]);
                return parsed;
            }
            let reason = _interp.get_attr(&resp, "reason")?;
            let headers = call_method(_interp, &resp, "getheaders", &[])
                .unwrap_or_else(|_| Value::list(Vec::new()));
            let _ = call_method(_interp, &resp, "read", &[]);
            let _ = call_method(_interp, self_val.0, "close", &[]);
            let host_str = host.0.as_str().unwrap_or_default();
            let pe_cls = xc_attr(_interp, "ProtocolError")?;
            let exc = call_pos(
                _interp,
                pe_cls,
                &[
                    Value::string(format!("{host_str}{}", handler.0)),
                    Value::int(status),
                    reason,
                    headers,
                ],
            )?;
            Err(PyError::raised(exc))
        }
    }

    class SafeTransport("Transport") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(false.into())] use_datetime: PyBool,
            #[default(false.into())] use_builtin_types: PyBool,
            #[default(OptionalArg::Omitted)] headers: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            let t_cls = xc_attr(_interp, "Transport")?;
            let t_init = _interp.get_attr(&t_cls, "__init__")?;
            let hdrs = headers.as_value().cloned().unwrap_or_else(Value::none);
            call_pos(
                _interp,
                t_init,
                &[
                    self_val.0.clone(),
                    Value::bool_(use_datetime.0),
                    Value::bool_(use_builtin_types.0),
                    hdrs,
                ],
            )?;
            let ctx = context.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "context", ctx)?;
            Ok(Value::none())
        }
    }

    #[native_state(MethodState)]
    class _Method {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            send: PyValue,
            name: PyStr<'_>,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                MethodState {
                    send: send.0,
                    name: name.0.to_string(),
                },
            )?;
            Ok(Value::none())
        }

        fn __getattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let (send, base) = with_state(self_val.0, |st| {
                Ok((st.send.clone(), st.name.clone()))
            })?;
            let m_cls = xc_attr(_interp, "_Method")?;
            call_pos(
                _interp,
                m_cls,
                &[send, Value::string(format!("{base}.{}", name.0))],
            )
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let (send, name) = with_state(self_val.0, |st| {
                Ok((st.send.clone(), st.name.clone()))
            })?;
            call_pos(
                _interp,
                send,
                &[Value::string(name), Value::tuple(args.to_vec())],
            )
        }
    }

    class ServerProxy {
        #[allow(clippy::too_many_arguments)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            uri: PyStr<'_>,
            #[default(OptionalArg::Omitted)] transport: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(false.into())] verbose: PyBool,
            #[default(false.into())] allow_none: PyBool,
            #[default(false.into())] use_datetime: PyBool,
            #[default(false.into())] use_builtin_types: PyBool,
            #[default(OptionalArg::Omitted)] headers: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            let (scheme, host, handler) = parse_xmlrpc_uri(&uri.0)?;
            let trans = match transport.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let hdrs = headers.as_value().cloned().unwrap_or_else(Value::none);
                    if scheme == "https" {
                        let st_cls = xc_attr(_interp, "SafeTransport")?;
                        let ctx = context.as_value().cloned().unwrap_or_else(Value::none);
                        call_pos(
                            _interp,
                            st_cls,
                            &[
                                Value::bool_(use_datetime.0),
                                Value::bool_(use_builtin_types.0),
                                hdrs,
                                ctx,
                            ],
                        )?
                    } else {
                        let t_cls = xc_attr(_interp, "Transport")?;
                        call_pos(
                            _interp,
                            t_cls,
                            &[
                                Value::bool_(use_datetime.0),
                                Value::bool_(use_builtin_types.0),
                                hdrs,
                            ],
                        )?
                    }
                }
            };
            let enc = match encoding.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => Value::string("utf-8"),
            };
            _interp.assign_attr(self_val.0.clone(), "_ServerProxy__host", Value::string(host))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_ServerProxy__handler",
                Value::string(handler),
            )?;
            _interp.assign_attr(self_val.0.clone(), "_ServerProxy__transport", trans)?;
            _interp.assign_attr(self_val.0.clone(), "_ServerProxy__encoding", enc)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_ServerProxy__verbose",
                Value::bool_(verbose.0),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_ServerProxy__allow_none",
                Value::bool_(allow_none.0),
            )?;
            Ok(Value::none())
        }

        fn _request(
            #[positional_only] self_val: PyValueRef<'_>,
            methodname: PyStr<'_>,
            params: PyValue,
        ) -> Result<Value> {
            let enc = _interp.get_attr(self_val.0, "_ServerProxy__encoding")?;
            let allow_none = _interp.get_attr(self_val.0, "_ServerProxy__allow_none")?;
            let dumps_fn = xc_attr(_interp, "dumps")?;
            let req_xml = call_pos(
                _interp,
                dumps_fn,
                &[
                    params.0,
                    Value::string(methodname.0.as_ref()),
                    Value::bool_(false),
                    enc.clone(),
                    allow_none,
                ],
            )?;
            let req_bytes = req_xml.as_str().unwrap_or_default().as_bytes().to_vec();
            let transport = _interp.get_attr(self_val.0, "_ServerProxy__transport")?;
            let host = _interp.get_attr(self_val.0, "_ServerProxy__host")?;
            let handler = _interp.get_attr(self_val.0, "_ServerProxy__handler")?;
            let verbose = _interp.get_attr(self_val.0, "_ServerProxy__verbose")?;
            let response = call_method(
                _interp,
                &transport,
                "request",
                &[host, handler, Value::bytes(req_bytes), verbose],
            )?;
            let items = _interp.collect_iterable(&response)?;
            if items.len() == 1 {
                return Ok(items[0].clone());
            }
            Ok(response)
        }

        fn _close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let transport = _interp.get_attr(self_val.0, "_ServerProxy__transport")?;
            call_method(_interp, &transport, "close", &[])
        }

        fn __getattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            if name.0.starts_with("__") && name.0.ends_with("__") {
                return Err(PyError::named(
                    "AttributeError",
                    format!("'ServerProxy' object has no attribute {:?}", name.0),
                ));
            }
            let send = _interp.get_attr(self_val.0, "_request")?;
            let m_cls = xc_attr(_interp, "_Method")?;
            call_pos(_interp, m_cls, &[send, Value::string(name.0.as_ref())])
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            attr: PyStr<'_>,
        ) -> Result<Value> {
            match attr.0.as_ref() {
                "close" => _interp.get_attr(self_val.0, "_close"),
                "transport" => _interp.get_attr(self_val.0, "_ServerProxy__transport"),
                _ => Err(PyError::named(
                    "ValueError",
                    "Attribute must be 'close' or 'transport'",
                )),
            }
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let host = _interp
                .get_attr(self_val.0, "_ServerProxy__host")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let handler = _interp
                .get_attr(self_val.0, "_ServerProxy__handler")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            Ok(Value::string(format!("<ServerProxy for {host}{handler}>")))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "__repr__", &[])
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            let _ = call_method(_interp, self_val.0, "_close", &[]);
            Ok(Value::none())
        }
    }

    #[native_state(MultiCallMethodState)]
    class _MultiCallMethod {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            call_list: PyValue,
            name: PyStr<'_>,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                MultiCallMethodState {
                    call_list: call_list.0,
                    name: name.0.to_string(),
                },
            )?;
            Ok(Value::none())
        }

        fn __getattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let (cl, base) = with_state(self_val.0, |st| {
                Ok((st.call_list.clone(), st.name.clone()))
            })?;
            let cls = xc_attr(_interp, "_MultiCallMethod")?;
            call_pos(
                _interp,
                cls,
                &[cl, Value::string(format!("{base}.{}", name.0))],
            )
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let (cl, name) = with_state(self_val.0, |st| {
                Ok((st.call_list.clone(), st.name.clone()))
            })?;
            call_method(
                _interp,
                &cl,
                "append",
                &[Value::tuple(vec![
                    Value::string(name),
                    Value::tuple(args.to_vec()),
                ])],
            )?;
            Ok(Value::none())
        }
    }

    class MultiCallIterator {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            results: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "results", results.0)?;
            Ok(Value::none())
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            i: PyValue,
        ) -> Result<Value> {
            let results = _interp.get_attr(self_val.0, "results")?;
            let item = _interp.eval_index(&results, i.0)?;
            if let Some(d) = item.as_dict() {
                let fc = d
                    .get(&PyKey::str_from("faultCode"))
                    .cloned()
                    .unwrap_or_else(|| Value::int(0));
                let fs = d
                    .get(&PyKey::str_from("faultString"))
                    .cloned()
                    .unwrap_or_else(|| Value::string(""));
                let fault_cls = xc_attr(_interp, "Fault")?;
                let exc = call_pos(_interp, fault_cls, &[fc, fs])?;
                return Err(PyError::raised(exc));
            }
            if matches!(item.kind(), ValueKind::List(_)) {
                let elems = _interp.collect_iterable(&item)?;
                if let Some(first) = elems.into_iter().next() {
                    return Ok(first);
                }
            }
            Err(PyError::named(
                "ValueError",
                "unexpected type in multicall result",
            ))
        }
    }

    class MultiCall {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            server: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_MultiCall__server", server.0)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_MultiCall__call_list",
                Value::list(Vec::new()),
            )?;
            Ok(Value::none())
        }

        fn __getattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            if name.0.starts_with("__") && name.0.ends_with("__") {
                return Err(PyError::named(
                    "AttributeError",
                    format!("'MultiCall' object has no attribute {:?}", name.0),
                ));
            }
            let cl = _interp.get_attr(self_val.0, "_MultiCall__call_list")?;
            let cls = xc_attr(_interp, "_MultiCallMethod")?;
            call_pos(_interp, cls, &[cl, Value::string(name.0.as_ref())])
        }

        fn __call__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cl = _interp.get_attr(self_val.0, "_MultiCall__call_list")?;
            let calls = _interp.collect_iterable(&cl)?;
            let mut marshalled = Vec::with_capacity(calls.len());
            for entry in calls {
                let pair = _interp.collect_iterable(&entry)?;
                let mut d = PyDict::default();
                d.insert(PyKey::str_from("methodName"), pair[0].clone());
                d.insert(PyKey::str_from("params"), pair[1].clone());
                marshalled.push(Value::dict(d));
            }
            let server = _interp.get_attr(self_val.0, "_MultiCall__server")?;
            let sys_proxy = _interp.get_attr(&server, "system")?;
            let mc_fn = _interp.get_attr(&sys_proxy, "multicall")?;
            let results = call_pos(_interp, mc_fn, &[Value::list(marshalled)])?;
            let iter_cls = xc_attr(_interp, "MultiCallIterator")?;
            call_pos(_interp, iter_cls, &[results])
        }
    }
}
