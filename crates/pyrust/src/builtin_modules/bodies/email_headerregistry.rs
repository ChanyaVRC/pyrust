// `email.headerregistry` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Structured RFC 5322 / MIME header objects and registry: `HeaderRegistry`,
// `BaseHeader`, `UnstructuredHeader`, `UniqueUnstructuredHeader`, `DateHeader`,
// `UniqueDateHeader`, `AddressHeader`, `UniqueAddressHeader`,
// `SingleAddressHeader`, `UniqueSingleAddressHeader`, `MIMEVersionHeader`,
// `ParameterizedMIMEHeader`, `ContentTypeHeader`, `ContentDispositionHeader`,
// `ContentTransferEncodingHeader`, `MessageIDHeader`, `Address`, and `Group`.
//
// Reference: <https://docs.python.org/3/library/email.headerregistry.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    call_kw(interp, callee, args, &[])
}

fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        if ch == '\\' || ch == '"' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

fn has_specials(s: &str) -> bool {
    s.bytes().any(|b| {
        matches!(
            b,
            b'(' | b')'
                | b'<'
                | b'>'
                | b'['
                | b']'
                | b':'
                | b';'
                | b'@'
                | b'\\'
                | b','
                | b'.'
                | b'"'
        )
    })
}

fn has_dot_atom_ends(s: &str) -> bool {
    s.bytes().any(|b| {
        matches!(
            b,
            b'(' | b')'
                | b'<'
                | b'>'
                | b'['
                | b']'
                | b':'
                | b';'
                | b'@'
                | b'\\'
                | b','
                | b'"'
                | b' '
                | b'\t'
        )
    }) || s.starts_with('.')
        || s.ends_with('.')
        || s.contains("..")
}

fn parse_addr_spec(addr_spec: &str) -> std::result::Result<(String, String), String> {
    let trimmed = addr_spec.trim();
    if trimmed.is_empty() {
        return Err(format!(
            "Invalid addr_spec; only '' could be parsed from '{addr_spec}'"
        ));
    }
    // Parse optional quoted local-part or dot-atom local-part, followed by @domain
    let (local_part, rest) = if let Some(after_q) = trimmed.strip_prefix('"') {
        let mut lp = String::new();
        let mut chars = after_q.char_indices();
        let mut closed = None;
        while let Some((idx, ch)) = chars.next() {
            if ch == '\\' {
                if let Some((_, next_ch)) = chars.next() {
                    lp.push(next_ch);
                } else {
                    return Err("unterminated quoted string".to_string());
                }
            } else if ch == '"' {
                closed = Some(idx);
                break;
            } else {
                lp.push(ch);
            }
        }
        let Some(end_idx) = closed else {
            return Err("unterminated quoted string".to_string());
        };
        (lp, &after_q[end_idx + 1..])
    } else {
        let Some(at_pos) = trimmed.find('@') else {
            return Err(format!("Invalid addr_spec '{addr_spec}'"));
        };
        let lp = &trimmed[..at_pos];
        if lp.is_empty()
            || lp
                .bytes()
                .any(|b| matches!(b, b' ' | b'\t' | b'<' | b'>' | b',' | b';' | b':'))
        {
            return Err(format!("Invalid addr_spec '{addr_spec}'"));
        }
        (lp.to_string(), &trimmed[at_pos..])
    };

    let Some(domain) = rest.strip_prefix('@') else {
        return Err(format!("Invalid addr_spec; missing '@' in '{addr_spec}'"));
    };
    if domain.is_empty()
        || domain
            .bytes()
            .any(|b| matches!(b, b' ' | b'\t' | b'<' | b'>' | b',' | b';' | b':' | b'@'))
    {
        return Err(format!(
            "Invalid addr_spec; invalid domain in '{addr_spec}'"
        ));
    }
    Ok((local_part, domain.to_string()))
}

fn decode_rfc2047_if_needed(interp: &mut Interpreter, value: &str) -> Result<String> {
    if !value.contains("=?") {
        return Ok(value.to_string());
    }
    let header_mod = interp.load_module("email.header")?;
    let decode_fn = interp.get_attr(&header_mod, "decode_header")?;
    let Ok(parts_val) = call_pos(interp, decode_fn, &[Value::string(value)]) else {
        return Ok(value.to_string());
    };
    let parts = interp.collect_iterable(&parts_val)?;
    let mut out = String::new();
    for pair in parts {
        let kv = interp.collect_iterable(&pair)?;
        if kv.len() < 2 {
            continue;
        }
        if let Some(s) = kv[0].as_str() {
            out.push_str(s);
        } else if matches!(kv[0].kind(), ValueKind::Bytes(_)) {
            let cs = kv[1].as_str().unwrap_or("ascii");
            let norm_cs = if cs.eq_ignore_ascii_case("us-ascii") || cs == "unknown-8bit" {
                "ascii"
            } else {
                cs
            };
            let dec_fn = interp.get_attr(&kv[0], "decode")?;
            let decoded = match call_pos(
                interp,
                dec_fn.clone(),
                &[Value::string(norm_cs), Value::string("replace")],
            ) {
                Ok(v) => v,
                Err(_) => call_pos(
                    interp,
                    dec_fn,
                    &[Value::string("ascii"), Value::string("replace")],
                )?,
            };
            out.push_str(decoded.as_str().unwrap_or(""));
        }
    }
    Ok(out)
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    let mut in_quote = false;
    let mut escape = false;
    for ch in s.chars() {
        if escape {
            if depth == 0 {
                out.push(ch);
            }
            escape = false;
            continue;
        }
        if ch == '\\' && (in_quote || depth > 0) {
            escape = true;
            if depth == 0 {
                out.push(ch);
            }
            continue;
        }
        if depth == 0 && ch == '"' {
            in_quote = !in_quote;
            out.push(ch);
            continue;
        }
        if !in_quote {
            if ch == '(' {
                depth += 1;
                continue;
            }
            if ch == ')' && depth > 0 {
                depth -= 1;
                continue;
            }
        }
        if depth == 0 {
            out.push(ch);
        }
    }
    out
}

fn split_unquoted(s: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut escape = false;
    let mut angle = 0usize;
    let mut paren = 0usize;
    for ch in s.chars() {
        if escape {
            cur.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' && (in_quote || paren > 0) {
            cur.push(ch);
            escape = true;
            continue;
        }
        if paren == 0 && ch == '"' {
            in_quote = !in_quote;
            cur.push(ch);
            continue;
        }
        if !in_quote {
            if ch == '(' {
                paren += 1;
            } else if ch == ')' && paren > 0 {
                paren -= 1;
            } else if paren == 0 && ch == '<' {
                angle += 1;
            } else if paren == 0 && ch == '>' && angle > 0 {
                angle -= 1;
            } else if ch == delim && paren == 0 && angle == 0 {
                out.push(std::mem::take(&mut cur));
                continue;
            }
        }
        cur.push(ch);
    }
    out.push(cur);
    out
}

fn unquote_str(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        let mut out = String::with_capacity(t.len() - 2);
        let mut chars = t[1..t.len() - 1].chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else {
                out.push(ch);
            }
        }
        out
    } else {
        t.to_string()
    }
}

fn parse_mime_params(value: &str) -> (String, IndexMap<String, String>) {
    let parts = split_unquoted(value, ';');
    let main = parts
        .first()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let mut params = IndexMap::new();
    for p in parts.into_iter().skip(1) {
        let trimmed = p.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(eq) = trimmed.find('=') {
            let k = trimmed[..eq].trim().to_ascii_lowercase();
            let v = unquote_str(&trimmed[eq + 1..]);
            params.insert(k, v);
        }
    }
    (main, params)
}

fn make_address_obj(
    interp: &mut Interpreter,
    display_name: &str,
    username: &str,
    domain: &str,
) -> Result<Value> {
    let m = interp.load_module("email.headerregistry")?;
    let cls = interp.get_attr(&m, "Address")?;
    call_pos(
        interp,
        cls,
        &[
            Value::string(display_name),
            Value::string(username),
            Value::string(domain),
        ],
    )
}

fn make_group_obj(
    interp: &mut Interpreter,
    display_name: Option<&str>,
    addresses: Vec<Value>,
) -> Result<Value> {
    let m = interp.load_module("email.headerregistry")?;
    let cls = interp.get_attr(&m, "Group")?;
    let dn = display_name.map(Value::string).unwrap_or_else(Value::none);
    call_pos(interp, cls, &[dn, Value::tuple(addresses)])
}

fn parse_single_mailbox(interp: &mut Interpreter, raw: &str) -> Result<Option<Value>> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let decoded = decode_rfc2047_if_needed(interp, s)?;
    let s = decoded.trim();
    if let Some(lt) = s.find('<')
        && let Some(gt) = s[lt + 1..].find('>')
    {
        let display = unquote_str(s[..lt].trim());
        let inside = s[lt + 1..lt + 1 + gt].trim();
        let (user, dom) = if let Some(at) = inside.rfind('@') {
            (unquote_str(&inside[..at]), inside[at + 1..].to_string())
        } else {
            (inside.to_string(), String::new())
        };
        return Ok(Some(make_address_obj(interp, &display, &user, &dom)?));
    }
    let clean = strip_comments(s);
    let clean = clean.trim();
    if clean.is_empty() {
        return Ok(None);
    }
    let (user, dom) = if let Some(at) = clean.rfind('@') {
        (unquote_str(&clean[..at]), clean[at + 1..].to_string())
    } else {
        (clean.to_string(), String::new())
    };
    Ok(Some(make_address_obj(interp, "", &user, &dom)?))
}

fn parse_address_list_groups(interp: &mut Interpreter, value: &str) -> Result<Vec<Value>> {
    let mut groups = Vec::new();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(groups);
    }
    // Split top-level entries, respecting group `name: a@b, c@d;` syntax.
    let mut cur = String::new();
    let mut in_quote = false;
    let mut escape = false;
    let mut in_group = false;
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut entries: Vec<(bool, String)> = Vec::new();

    for ch in trimmed.chars() {
        if escape {
            cur.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' && (in_quote || paren > 0) {
            cur.push(ch);
            escape = true;
            continue;
        }
        if paren == 0 && ch == '"' {
            in_quote = !in_quote;
            cur.push(ch);
            continue;
        }
        if !in_quote {
            if ch == '(' {
                paren += 1;
            } else if ch == ')' && paren > 0 {
                paren -= 1;
            } else if paren == 0 && ch == '<' {
                angle += 1;
            } else if paren == 0 && ch == '>' && angle > 0 {
                angle -= 1;
            } else if paren == 0 && angle == 0 && ch == ':' && !in_group {
                in_group = true;
                cur.push(ch);
                continue;
            } else if paren == 0 && angle == 0 && ch == ';' && in_group {
                in_group = false;
                entries.push((true, std::mem::take(&mut cur)));
                continue;
            } else if paren == 0 && angle == 0 && ch == ',' && !in_group {
                let s = std::mem::take(&mut cur);
                if !s.trim().is_empty() {
                    entries.push((false, s));
                }
                continue;
            }
        }
        cur.push(ch);
    }
    if !cur.trim().is_empty() {
        entries.push((in_group, cur));
    }

    for (is_grp, entry) in entries {
        if is_grp {
            if let Some(colon) = entry.find(':') {
                let gname = unquote_str(entry[..colon].trim());
                let body = &entry[colon + 1..];
                let mut addrs = Vec::new();
                for part in split_unquoted(body, ',') {
                    if let Some(a) = parse_single_mailbox(interp, &part)? {
                        addrs.push(a);
                    }
                }
                groups.push(make_group_obj(interp, Some(&gname), addrs)?);
            }
        } else if let Some(a) = parse_single_mailbox(interp, &entry)? {
            groups.push(make_group_obj(interp, None, vec![a])?);
        }
    }
    Ok(groups)
}

fn default_header_map(interp: &mut Interpreter, mod_val: &Value) -> Result<Value> {
    let mut d = PyDict::default();
    let entries = [
        ("subject", "UniqueUnstructuredHeader"),
        ("date", "UniqueDateHeader"),
        ("resent-date", "DateHeader"),
        ("orig-date", "UniqueDateHeader"),
        ("sender", "UniqueSingleAddressHeader"),
        ("resent-sender", "SingleAddressHeader"),
        ("to", "UniqueAddressHeader"),
        ("resent-to", "AddressHeader"),
        ("cc", "UniqueAddressHeader"),
        ("resent-cc", "AddressHeader"),
        ("bcc", "UniqueAddressHeader"),
        ("resent-bcc", "AddressHeader"),
        ("from", "UniqueAddressHeader"),
        ("resent-from", "AddressHeader"),
        ("reply-to", "UniqueAddressHeader"),
        ("mime-version", "MIMEVersionHeader"),
        ("content-type", "ContentTypeHeader"),
        ("content-disposition", "ContentDispositionHeader"),
        ("content-transfer-encoding", "ContentTransferEncodingHeader"),
        ("message-id", "MessageIDHeader"),
    ];
    for (k, cls_name) in entries {
        let cls = interp.get_attr(mod_val, cls_name)?;
        d.insert(PyKey::str_from(k), cls);
    }
    Ok(Value::dict(d))
}

fn dict_pop_key(
    _interp: &mut Interpreter,
    kw: &mut IndexMap<String, Value>,
    key: &str,
) -> Result<Value> {
    kw.shift_remove(key)
        .ok_or_else(|| PyError::named("KeyError", key.to_string()))
}

fn call_super_init(
    interp: &mut Interpreter,
    self_val: &Value,
    name: Value,
    kw: IndexMap<String, Value>,
) -> Result<Value> {
    let m = interp.load_module("email.headerregistry")?;
    let bh = interp.get_attr(&m, "BaseHeader")?;
    let init_fn = interp.get_attr(&bh, "init")?;
    let mut expanded = vec![
        ExpandedCallArg {
            name: None,
            value: self_val.clone(),
        },
        ExpandedCallArg {
            name: None,
            value: name,
        },
    ];
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some(k),
            value: v,
        });
    }
    interp.call_function_expanded(init_fn, &expanded)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let mod_val = Value::py_module(Rc::clone(module));
    let dmap = default_header_map(interp, &mod_val)?;
    module
        .borrow_mut()
        .attrs
        .insert("_default_header_map".to_string(), dmap);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email"),
    }

    class Address {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("".into())] display_name: PyStr<'_>,
            #[default("".into())] username: PyStr<'_>,
            #[default("".into())] domain: PyStr<'_>,
            #[default(OptionalArg::Omitted)] addr_spec: OptValueRef<'_>,
        ) -> Result<Value> {
            let dn = display_name.0.as_ref();
            let mut user = username.0.to_string();
            let mut dom = domain.0.to_string();
            let as_opt = addr_spec
                .as_value()
                .filter(|v| !v.is_none())
                .and_then(|v| v.as_str());

            let all_inputs = [dn, user.as_str(), dom.as_str(), as_opt.unwrap_or("")];
            if all_inputs.iter().any(|s| s.contains('\r') || s.contains('\n')) {
                return Err(PyError::named(
                    "ValueError",
                    "invalid arguments; address parts cannot contain CR or LF".to_string(),
                ));
            }

            if let Some(spec) = as_opt {
                if !user.is_empty() || !dom.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        "addrspec specified when username and/or domain also specified".to_string(),
                    ));
                }
                match parse_addr_spec(spec) {
                    Ok((u, d)) => {
                        user = u;
                        dom = d;
                    }
                    Err(msg) => {
                        return Err(PyError::named("ValueError", msg));
                    }
                }
            }

            _interp.assign_attr(self_val.0.clone(), "_display_name", Value::string(dn))?;
            _interp.assign_attr(self_val.0.clone(), "_username", Value::string(user))?;
            _interp.assign_attr(self_val.0.clone(), "_domain", Value::string(dom))?;
            Ok(Value::none())
        }

        #[getter]
        fn display_name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_display_name")
        }

        #[getter]
        fn username(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_username")
        }

        #[getter]
        fn domain(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_domain")
        }

        #[getter]
        fn addr_spec(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let user_v = _interp.get_attr(self_val.0, "_username")?;
            let dom_v = _interp.get_attr(self_val.0, "_domain")?;
            let user = user_v.as_str().unwrap_or("");
            let dom = dom_v.as_str().unwrap_or("");
            let lp = if !user.is_empty() && has_dot_atom_ends(user) {
                quote_string(user)
            } else {
                user.to_string()
            };
            if !dom.is_empty() {
                Ok(Value::string(format!("{lp}@{dom}")))
            } else if lp.is_empty() {
                Ok(Value::string("<>"))
            } else {
                Ok(Value::string(lp))
            }
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = _interp.get_attr(self_val.0, "__class__")?;
            let cname = _interp
                .get_attr(&cls, "__name__")?
                .as_str()
                .unwrap_or("Address")
                .to_string();
            let dn = _interp.get_attr(self_val.0, "display_name")?;
            let un = _interp.get_attr(self_val.0, "username")?;
            let dm = _interp.get_attr(self_val.0, "domain")?;
            let repr_fn = Value::builtin_function("repr");
            let dn_r = call_pos(_interp, repr_fn.clone(), &[dn])?;
            let un_r = call_pos(_interp, repr_fn.clone(), &[un])?;
            let dm_r = call_pos(_interp, repr_fn, &[dm])?;
            Ok(Value::string(format!(
                "{cname}(display_name={}, username={}, domain={})",
                dn_r.as_str().unwrap_or("''"),
                un_r.as_str().unwrap_or("''"),
                dm_r.as_str().unwrap_or("''"),
            )))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let dn_v = _interp.get_attr(self_val.0, "display_name")?;
            let dn = dn_v.as_str().unwrap_or("");
            let as_v = _interp.get_attr(self_val.0, "addr_spec")?;
            let spec = as_v.as_str().unwrap_or("<>");
            if !dn.is_empty() {
                let disp = if has_specials(dn) {
                    quote_string(dn)
                } else {
                    dn.to_string()
                };
                let shown = if spec == "<>" { "" } else { spec };
                Ok(Value::string(format!("{disp} <{shown}>")))
            } else {
                Ok(Value::string(spec))
            }
        }

        fn __eq__(
            #[positional_only] self_val: PyValueRef<'_>,
            other: PyValueRef<'_>,
        ) -> Result<Value> {
            let m = _interp.load_module("email.headerregistry")?;
            let cls = _interp.get_attr(&m, "Address")?;
            let is_addr = call_pos(
                _interp,
                Value::builtin_function("isinstance"),
                &[other.0.clone(), cls],
            )?;
            if !_interp.truthy_value(&is_addr)? {
                return Ok(Value::not_implemented());
            }
            for attr in ["display_name", "username", "domain"] {
                let a = _interp.get_attr(self_val.0, attr)?;
                let b = _interp.get_attr(other.0, attr)?;
                let eq_fn = _interp.get_attr(&a, "__eq__")?;
                let eq_v = call_pos(_interp, eq_fn, &[b])?;
                if !_interp.truthy_value(&eq_v)? {
                    return Ok(Value::bool_(false));
                }
            }
            Ok(Value::bool_(true))
        }
    }

    class Group {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] display_name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] addresses: OptValueRef<'_>,
        ) -> Result<Value> {
            let dn = display_name.as_value().cloned().unwrap_or_else(Value::none);
            let addrs = match addresses.as_value() {
                Some(v) if !v.is_none() && _interp.truthy_value(v)? => {
                    Value::tuple(_interp.collect_iterable(v)?)
                }
                _ => Value::tuple(Vec::new()),
            };
            _interp.assign_attr(self_val.0.clone(), "_display_name", dn)?;
            _interp.assign_attr(self_val.0.clone(), "_addresses", addrs)?;
            Ok(Value::none())
        }

        #[getter]
        fn display_name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_display_name")
        }

        #[getter]
        fn addresses(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_addresses")
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            // Note: CPython 3.12's Group.__repr__ intentionally omits the trailing ')'.
            let cls = _interp.get_attr(self_val.0, "__class__")?;
            let cname = _interp
                .get_attr(&cls, "__name__")?
                .as_str()
                .unwrap_or("Group")
                .to_string();
            let dn = _interp.get_attr(self_val.0, "display_name")?;
            let addrs = _interp.get_attr(self_val.0, "addresses")?;
            let repr_fn = Value::builtin_function("repr");
            let dn_r = call_pos(_interp, repr_fn.clone(), &[dn])?;
            let ad_r = call_pos(_interp, repr_fn, &[addrs])?;
            Ok(Value::string(format!(
                "{cname}(display_name={}, addresses={}",
                dn_r.as_str().unwrap_or("None"),
                ad_r.as_str().unwrap_or("()"),
            )))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let dn = _interp.get_attr(self_val.0, "display_name")?;
            let addrs_v = _interp.get_attr(self_val.0, "addresses")?;
            let addrs = _interp.collect_iterable(&addrs_v)?;
            if dn.is_none() && addrs.len() == 1 {
                return Ok(Value::string(super::email_errors::py_str(_interp, &addrs[0])?));
            }
            let disp = if dn.is_none() {
                "None".to_string()
            } else {
                let s = dn.as_str().unwrap_or("");
                if has_specials(s) {
                    quote_string(s)
                } else {
                    s.to_string()
                }
            };
            let mut parts = Vec::with_capacity(addrs.len());
            for a in addrs {
                parts.push(super::email_errors::py_str(_interp, &a)?);
            }
            let joined = parts.join(", ");
            if joined.is_empty() {
                Ok(Value::string(format!("{disp}:;")))
            } else {
                Ok(Value::string(format!("{disp}: {joined};")))
            }
        }

        fn __eq__(
            #[positional_only] self_val: PyValueRef<'_>,
            other: PyValueRef<'_>,
        ) -> Result<Value> {
            let m = _interp.load_module("email.headerregistry")?;
            let cls = _interp.get_attr(&m, "Group")?;
            let is_grp = call_pos(
                _interp,
                Value::builtin_function("isinstance"),
                &[other.0.clone(), cls],
            )?;
            if !_interp.truthy_value(&is_grp)? {
                return Ok(Value::not_implemented());
            }
            for attr in ["display_name", "addresses"] {
                let a = _interp.get_attr(self_val.0, attr)?;
                let b = _interp.get_attr(other.0, attr)?;
                let eq_fn = _interp.get_attr(&a, "__eq__")?;
                let eq_v = call_pos(_interp, eq_fn, &[b])?;
                if !_interp.truthy_value(&eq_v)? {
                    return Ok(Value::bool_(false));
                }
            }
            Ok(Value::bool_(true))
        }
    }

    class BaseHeader("str") {
        fn __new__(
            #[positional_only] cls: PyValueRef<'_>,
            name: PyValue,
            value: PyValue,
        ) -> Result<Value> {
            let mut kwds_dict = PyDict::default();
            kwds_dict.insert(PyKey::str_from("defects"), Value::list(Vec::new()));
            let kwds_val = Value::dict(kwds_dict);
            let parse_fn = _interp.get_attr(cls.0, "parse")?;
            call_pos(_interp, parse_fn, &[value.0, kwds_val.clone()])?;

            let mut decoded = kwds_val
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from("decoded")).cloned())
                .unwrap_or_else(|| Value::string(""));

            if let Ok(utils) = _interp.load_module("email.utils")
                && let Ok(hs) = _interp.get_attr(&utils, "_has_surrogates")
                && call_pos(_interp, hs, std::slice::from_ref(&decoded))
                    .and_then(|v| _interp.truthy_value(&v))
                    .unwrap_or(false)
                && let Ok(san) = _interp.get_attr(&utils, "_sanitize")
            {
                decoded = call_pos(_interp, san, &[decoded])?;
            }

            let str_cls = Value::py_class(
                crate::interpreter::primitive_class_by_name("str")
                    .ok_or_else(|| PyError::runtime("str class not found".to_string()))?,
            );
            let str_new = _interp.get_attr(&str_cls, "__new__")?;
            let inst = call_pos(_interp, str_new, &[cls.0.clone(), decoded])?;

            let mut init_args = vec![ExpandedCallArg {
                name: None,
                value: name.0,
            }];
            if let Some(d) = kwds_val.as_dict() {
                for (k, v) in d.iter() {
                    if let PyKey::Str(ks) = k
                        && let Some(s) = ks.as_str()
                        && s != "decoded"
                    {
                        init_args.push(ExpandedCallArg {
                            name: Some(s.to_string()),
                            value: v.clone(),
                        });
                    }
                }
            }
            let init_fn = _interp.get_attr(&inst, "init")?;
            _interp.call_function_expanded(init_fn, &init_args)?;
            Ok(inst)
        }

        fn init(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            #[keyword_only] parse_tree: PyValue,
            #[keyword_only] defects: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_name", name.0)?;
            _interp.assign_attr(self_val.0.clone(), "_parse_tree", parse_tree.0)?;
            _interp.assign_attr(self_val.0.clone(), "_defects", defects.0)?;
            Ok(Value::none())
        }

        #[getter]
        fn name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_name")
        }

        #[getter]
        fn defects(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let d = _interp.get_attr(self_val.0, "_defects")?;
            let items = _interp.collect_iterable(&d)?;
            Ok(Value::tuple(items))
        }

        fn fold(
            #[positional_only] self_val: PyValueRef<'_>,
            #[keyword_only] policy: PyValue,
        ) -> Result<Value> {
            let name = _interp.get_attr(self_val.0, "name")?;
            let val_str = super::email_errors::py_str(_interp, self_val.0)?;
            let fold_fn = _interp.get_attr(&policy.0, "fold")?;
            call_pos(_interp, fold_fn, &[name, Value::string(val_str)])
        }
    }

    class UnstructuredHeader {
        constants {
            "max_count" => Value::none(),
        }

        #[classmethod]
        fn parse(
            #[positional_only] _cls: PyValueRef<'_>,
            value: PyValue,
            kwds: PyValue,
        ) -> Result<Value> {
            let s = super::email_errors::py_str(_interp, &value.0)?;
            let decoded = decode_rfc2047_if_needed(_interp, &s)?;
            kwds.0.dict_insert(PyKey::str_from("parse_tree"), Value::string(&decoded))?;
            kwds.0.dict_insert(PyKey::str_from("decoded"), Value::string(&decoded))?;
            Ok(Value::none())
        }
    }

    class UniqueUnstructuredHeader("UnstructuredHeader") {
        constants {
            "max_count" => Value::int(1),
        }
    }

    class DateHeader {
        constants {
            "max_count" => Value::none(),
        }

        #[classmethod]
        fn parse(
            #[positional_only] _cls: PyValueRef<'_>,
            value: PyValue,
            kwds: PyValue,
        ) -> Result<Value> {
            if !_interp.truthy_value(&value.0)? {
                let defect =
                    super::email_errors::make_defect(_interp, "HeaderMissingRequiredValue", None)?;
                if let Some(d_list) = kwds
                    .0
                    .as_dict()
                    .and_then(|d| d.get(&PyKey::str_from("defects")).cloned())
                {
                    let app = _interp.get_attr(&d_list, "append")?;
                    call_pos(_interp, app, &[defect])?;
                }
                kwds.0.dict_insert(PyKey::str_from("datetime"), Value::none())?;
                kwds.0.dict_insert(PyKey::str_from("decoded"), Value::string(""))?;
                kwds.0.dict_insert(PyKey::str_from("parse_tree"), Value::string(""))?;
                return Ok(Value::none());
            }

            let utils = _interp.load_module("email.utils")?;
            let dt_val = if let Some(s) = value.0.as_str() {
                kwds.0.dict_insert(PyKey::str_from("decoded"), Value::string(s))?;
                let p2d = _interp.get_attr(&utils, "parsedate_to_datetime")?;
                match call_pos(_interp, p2d, &[Value::string(s)]) {
                    Ok(dt) => dt,
                    Err(e) if e.class_name_is("ValueError") => {
                        let defect = super::email_errors::make_defect(
                            _interp,
                            "InvalidDateDefect",
                            Some(Value::string("Invalid date value or format")),
                        )?;
                        if let Some(d_list) = kwds
                            .0
                            .as_dict()
                            .and_then(|d| d.get(&PyKey::str_from("defects")).cloned())
                        {
                            let app = _interp.get_attr(&d_list, "append")?;
                            call_pos(_interp, app, &[defect])?;
                        }
                        kwds.0.dict_insert(PyKey::str_from("datetime"), Value::none())?;
                        kwds.0.dict_insert(PyKey::str_from("parse_tree"), Value::string(""))?;
                        return Ok(Value::none());
                    }
                    Err(e) => return Err(e),
                }
            } else {
                value.0
            };

            let fmt_dt = _interp.get_attr(&utils, "format_datetime")?;
            let formatted = call_pos(_interp, fmt_dt, std::slice::from_ref(&dt_val))?;
            kwds.0.dict_insert(PyKey::str_from("datetime"), dt_val)?;
            kwds.0.dict_insert(PyKey::str_from("decoded"), formatted.clone())?;
            kwds.0.dict_insert(PyKey::str_from("parse_tree"), formatted)?;
            Ok(Value::none())
        }

        fn init(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut kw = kw;
            let dt = dict_pop_key(_interp, &mut kw, "datetime")?;
            _interp.assign_attr(self_val.0.clone(), "_datetime", dt)?;
            call_super_init(_interp, self_val.0, name.0, kw)
        }

        #[getter]
        fn datetime(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_datetime")
        }
    }

    class UniqueDateHeader("DateHeader") {
        constants {
            "max_count" => Value::int(1),
        }
    }

    class AddressHeader {
        constants {
            "max_count" => Value::none(),
        }

        #[classmethod]
        fn parse(
            #[positional_only] _cls: PyValueRef<'_>,
            value: PyValue,
            kwds: PyValue,
        ) -> Result<Value> {
            let groups = if let Some(s) = value.0.as_str() {
                parse_address_list_groups(_interp, s)?
            } else {
                let items = if _interp.get_attr(&value.0, "__iter__").is_ok() {
                    _interp.collect_iterable(&value.0)?
                } else {
                    vec![value.0]
                };
                let mut gs = Vec::with_capacity(items.len());
                for item in items {
                    if _interp.get_attr(&item, "addresses").is_ok() {
                        gs.push(item);
                    } else {
                        gs.push(make_group_obj(_interp, None, vec![item])?);
                    }
                }
                gs
            };
            let mut rendered = Vec::with_capacity(groups.len());
            for g in &groups {
                rendered.push(super::email_errors::py_str(_interp, g)?);
            }
            let decoded = rendered.join(", ");
            kwds.0.dict_insert(PyKey::str_from("groups"), Value::list(groups))?;
            kwds.0.dict_insert(PyKey::str_from("decoded"), Value::string(&decoded))?;
            kwds.0.dict_insert(PyKey::str_from("parse_tree"), Value::string(&decoded))?;
            Ok(Value::none())
        }

        fn init(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut kw = kw;
            let groups = dict_pop_key(_interp, &mut kw, "groups")?;
            let g_tuple = Value::tuple(_interp.collect_iterable(&groups)?);
            _interp.assign_attr(self_val.0.clone(), "_groups", g_tuple)?;
            _interp.assign_attr(self_val.0.clone(), "_addresses", Value::none())?;
            call_super_init(_interp, self_val.0, name.0, kw)
        }

        #[getter]
        fn groups(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_groups")
        }

        #[getter]
        fn addresses(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cached = _interp.get_attr(self_val.0, "_addresses")?;
            if !cached.is_none() {
                return Ok(cached);
            }
            let groups = _interp.get_attr(self_val.0, "_groups")?;
            let mut all = Vec::new();
            for g in _interp.collect_iterable(&groups)? {
                let g_addrs = _interp.get_attr(&g, "addresses")?;
                all.extend(_interp.collect_iterable(&g_addrs)?);
            }
            let tup = Value::tuple(all);
            _interp.assign_attr(self_val.0.clone(), "_addresses", tup.clone())?;
            Ok(tup)
        }
    }

    class UniqueAddressHeader("AddressHeader") {
        constants {
            "max_count" => Value::int(1),
        }
    }

    class SingleAddressHeader("AddressHeader") {
        #[getter]
        fn address(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let addrs_v = _interp.get_attr(self_val.0, "addresses")?;
            let addrs = _interp.collect_iterable(&addrs_v)?;
            if addrs.len() != 1 {
                let name = _interp.get_attr(self_val.0, "name")?;
                return Err(PyError::named(
                    "ValueError",
                    format!(
                        "value of single address header {} is not a single address",
                        super::email_errors::py_str(_interp, &name)?
                    ),
                ));
            }
            Ok(addrs.into_iter().next().unwrap())
        }
    }

    class UniqueSingleAddressHeader("SingleAddressHeader") {
        constants {
            "max_count" => Value::int(1),
        }
    }

    class MIMEVersionHeader {
        constants {
            "max_count" => Value::int(1),
        }

        #[classmethod]
        fn parse(
            #[positional_only] _cls: PyValueRef<'_>,
            value: PyValue,
            kwds: PyValue,
        ) -> Result<Value> {
            let raw = super::email_errors::py_str(_interp, &value.0)?;
            let clean = strip_comments(&raw);
            let trimmed = clean.trim();
            let mut major: Option<i64> = None;
            let mut minor: Option<i64> = None;
            if let Some((a, b)) = trimmed.split_once('.')
                && let (Ok(maj), Ok(min)) = (a.trim().parse::<i64>(), b.trim().parse::<i64>())
            {
                major = Some(maj);
                minor = Some(min);
            } else {
                let defect = super::email_errors::make_defect(
                    _interp,
                    "InvalidHeaderDefect",
                    Some(Value::string("Expected MIME version")),
                )?;
                if let Some(d_list) = kwds
                    .0
                    .as_dict()
                    .and_then(|d| d.get(&PyKey::str_from("defects")).cloned())
                {
                    let app = _interp.get_attr(&d_list, "append")?;
                    call_pos(_interp, app, &[defect])?;
                }
            }
            let version = match (major, minor) {
                (Some(maj), Some(min)) => Value::string(format!("{maj}.{min}")),
                _ => Value::none(),
            };
            kwds.0.dict_insert(PyKey::str_from("decoded"), Value::string(&raw))?;
            kwds.0.dict_insert(PyKey::str_from("parse_tree"), Value::string(&raw))?;
            kwds.0.dict_insert(
                PyKey::str_from("major"),
                major.map(Value::int).unwrap_or_else(Value::none),
            )?;
            kwds.0.dict_insert(
                PyKey::str_from("minor"),
                minor.map(Value::int).unwrap_or_else(Value::none),
            )?;
            kwds.0.dict_insert(PyKey::str_from("version"), version)?;
            Ok(Value::none())
        }

        fn init(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut kw = kw;
            let version = dict_pop_key(_interp, &mut kw, "version")?;
            let major = dict_pop_key(_interp, &mut kw, "major")?;
            let minor = dict_pop_key(_interp, &mut kw, "minor")?;
            _interp.assign_attr(self_val.0.clone(), "_version", version)?;
            _interp.assign_attr(self_val.0.clone(), "_major", major)?;
            _interp.assign_attr(self_val.0.clone(), "_minor", minor)?;
            call_super_init(_interp, self_val.0, name.0, kw)
        }

        #[getter]
        fn major(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_major")
        }

        #[getter]
        fn minor(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_minor")
        }

        #[getter]
        fn version(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_version")
        }
    }

    class ParameterizedMIMEHeader {
        constants {
            "max_count" => Value::int(1),
        }

        #[classmethod]
        fn parse(
            #[positional_only] _cls: PyValueRef<'_>,
            value: PyValue,
            kwds: PyValue,
        ) -> Result<Value> {
            let raw = super::email_errors::py_str(_interp, &value.0)?;
            let (_main, params) = parse_mime_params(&raw);
            let mut pdict = PyDict::default();
            for (k, v) in params {
                pdict.insert(PyKey::str_from(&k), Value::string(v));
            }
            kwds.0.dict_insert(PyKey::str_from("decoded"), Value::string(&raw))?;
            kwds.0.dict_insert(PyKey::str_from("parse_tree"), Value::string(&raw))?;
            kwds.0.dict_insert(PyKey::str_from("params"), Value::dict(pdict))?;
            Ok(Value::none())
        }

        fn init(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut kw = kw;
            let params = dict_pop_key(_interp, &mut kw, "params")?;
            _interp.assign_attr(self_val.0.clone(), "_params", params)?;
            call_super_init(_interp, self_val.0, name.0, kw)
        }

        #[getter]
        fn params(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let p = _interp.get_attr(self_val.0, "_params")?;
            let types_mod = _interp.load_module("types")?;
            let mp_cls = _interp.get_attr(&types_mod, "MappingProxyType")?;
            call_pos(_interp, mp_cls, &[p])
        }
    }

    class ContentTypeHeader("ParameterizedMIMEHeader") {
        fn init(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let m = _interp.load_module("email.headerregistry")?;
            let pmh = _interp.get_attr(&m, "ParameterizedMIMEHeader")?;
            let init_fn = _interp.get_attr(&pmh, "init")?;
            let mut expanded = vec![
                ExpandedCallArg {
                    name: None,
                    value: self_val.0.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: name.0,
                },
            ];
            for (k, v) in kw {
                expanded.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            _interp.call_function_expanded(init_fn, &expanded)?;
            let raw = super::email_errors::py_str(_interp, self_val.0)?;
            let (main, _params) = parse_mime_params(&raw);
            let (maintype, subtype) = if let Some((a, b)) = main.split_once('/') {
                (a.trim().to_ascii_lowercase(), b.trim().to_ascii_lowercase())
            } else {
                ("text".to_string(), "plain".to_string())
            };
            _interp.assign_attr(self_val.0.clone(), "_maintype", Value::string(maintype))?;
            _interp.assign_attr(self_val.0.clone(), "_subtype", Value::string(subtype))?;
            Ok(Value::none())
        }

        #[getter]
        fn maintype(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_maintype")
        }

        #[getter]
        fn subtype(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_subtype")
        }

        #[getter]
        fn content_type(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mt = _interp.get_attr(self_val.0, "_maintype")?;
            let st = _interp.get_attr(self_val.0, "_subtype")?;
            Ok(Value::string(format!(
                "{}/{}",
                mt.as_str().unwrap_or("text"),
                st.as_str().unwrap_or("plain")
            )))
        }
    }

    class ContentDispositionHeader("ParameterizedMIMEHeader") {
        fn init(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let m = _interp.load_module("email.headerregistry")?;
            let pmh = _interp.get_attr(&m, "ParameterizedMIMEHeader")?;
            let init_fn = _interp.get_attr(&pmh, "init")?;
            let mut expanded = vec![
                ExpandedCallArg {
                    name: None,
                    value: self_val.0.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: name.0,
                },
            ];
            for (k, v) in kw {
                expanded.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            _interp.call_function_expanded(init_fn, &expanded)?;
            let raw = super::email_errors::py_str(_interp, self_val.0)?;
            let (main, _params) = parse_mime_params(&raw);
            let cd = if main.is_empty() {
                Value::none()
            } else {
                Value::string(main.to_ascii_lowercase())
            };
            _interp.assign_attr(self_val.0.clone(), "_content_disposition", cd)?;
            Ok(Value::none())
        }

        #[getter]
        fn content_disposition(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_content_disposition")
        }
    }

    class ContentTransferEncodingHeader {
        constants {
            "max_count" => Value::int(1),
        }

        #[classmethod]
        fn parse(
            #[positional_only] _cls: PyValueRef<'_>,
            value: PyValue,
            kwds: PyValue,
        ) -> Result<Value> {
            let raw = super::email_errors::py_str(_interp, &value.0)?;
            kwds.0.dict_insert(PyKey::str_from("decoded"), Value::string(&raw))?;
            kwds.0.dict_insert(PyKey::str_from("parse_tree"), Value::string(&raw))?;
            Ok(Value::none())
        }

        fn init(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_super_init(_interp, self_val.0, name.0, kw)?;
            let raw = super::email_errors::py_str(_interp, self_val.0)?;
            let cte = strip_comments(&raw).trim().to_ascii_lowercase();
            _interp.assign_attr(self_val.0.clone(), "_cte", Value::string(cte))?;
            Ok(Value::none())
        }

        #[getter]
        fn cte(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_cte")
        }
    }

    class MessageIDHeader {
        constants {
            "max_count" => Value::int(1),
        }

        #[classmethod]
        fn parse(
            #[positional_only] _cls: PyValueRef<'_>,
            value: PyValue,
            kwds: PyValue,
        ) -> Result<Value> {
            let raw = super::email_errors::py_str(_interp, &value.0)?;
            let trimmed = raw.trim();
            if !(trimmed.starts_with('<') && trimmed.ends_with('>') && trimmed.contains('@')) {
                let defect = super::email_errors::make_defect(
                    _interp,
                    "InvalidHeaderDefect",
                    Some(Value::string("Invalid Message-ID")),
                )?;
                if let Some(d_list) = kwds
                    .0
                    .as_dict()
                    .and_then(|d| d.get(&PyKey::str_from("defects")).cloned())
                {
                    let app = _interp.get_attr(&d_list, "append")?;
                    call_pos(_interp, app, &[defect])?;
                }
            }
            kwds.0.dict_insert(PyKey::str_from("decoded"), Value::string(&raw))?;
            kwds.0.dict_insert(PyKey::str_from("parse_tree"), Value::string(&raw))?;
            Ok(Value::none())
        }
    }

    class HeaderRegistry {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] base_class: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] default_class: OptValueRef<'_>,
            #[default(true.into())] use_default_map: PyBool,
        ) -> Result<Value> {
            let m = _interp.load_module("email.headerregistry")?;
            let bc = match base_class.as_value() {
                Some(v) => v.clone(),
                None => _interp.get_attr(&m, "BaseHeader")?,
            };
            let dc = match default_class.as_value() {
                Some(v) => v.clone(),
                None => _interp.get_attr(&m, "UnstructuredHeader")?,
            };
            let reg = Value::dict(PyDict::default());
            if use_default_map.0 {
                let dmap = _interp.get_attr(&m, "_default_header_map")?;
                let upd = _interp.get_attr(&reg, "update")?;
                call_pos(_interp, upd, &[dmap])?;
            }
            _interp.assign_attr(self_val.0.clone(), "registry", reg)?;
            _interp.assign_attr(self_val.0.clone(), "base_class", bc)?;
            _interp.assign_attr(self_val.0.clone(), "default_class", dc)?;
            Ok(Value::none())
        }

        fn map_to_type(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            cls: PyValue,
        ) -> Result<Value> {
            let reg = _interp.get_attr(self_val.0, "registry")?;
            reg.dict_insert(PyKey::str_from(name.0.to_ascii_lowercase()), cls.0)?;
            Ok(Value::none())
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let reg = _interp.get_attr(self_val.0, "registry")?;
            let default_cls = _interp.get_attr(self_val.0, "default_class")?;
            let base_cls = _interp.get_attr(self_val.0, "base_class")?;
            let key = name.0.to_ascii_lowercase();
            let cls = reg
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(&key)).cloned())
                .unwrap_or(default_cls);
            let cls_name = _interp
                .get_attr(&cls, "__name__")?
                .as_str()
                .unwrap_or("Header")
                .to_string();
            let dyn_name = format!("_{cls_name}");
            let mut ns = PyDict::default();
            ns.insert(
                PyKey::str_from("__module__"),
                Value::string("email.headerregistry"),
            );
            call_pos(
                _interp,
                Value::builtin_function("type"),
                &[
                    Value::string(dyn_name),
                    Value::tuple(vec![cls, base_cls]),
                    Value::dict(ns),
                ],
            )
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            value: PyValue,
        ) -> Result<Value> {
            let getitem = _interp.get_attr(self_val.0, "__getitem__")?;
            let hdr_cls = call_pos(_interp, getitem, &[Value::string(name.0.as_ref())])?;
            call_pos(_interp, hdr_cls, &[Value::string(name.0.as_ref()), value.0])
        }
    }
}
