/// Render a `bytes` value the way Python does (b'...' with escapes).
fn bytes_repr(bytes: &[u8]) -> String {
    // Choose a quote: if any single quote and no double quote, use double; else single.
    let has_single = bytes.contains(&b'\'');
    let has_double = bytes.contains(&b'"');
    let q = if has_single && !has_double { '"' } else { '\'' };
    let mut out = String::with_capacity(bytes.len() + 3);
    out.push('b');
    out.push(q);
    for &b in bytes {
        match b {
            0x09 => out.push_str("\\t"),
            0x0a => out.push_str("\\n"),
            0x0d => out.push_str("\\r"),
            0x5c => out.push_str("\\\\"),
            b'\'' if q == '\'' => out.push_str("\\'"),
            b'"' if q == '"' => out.push_str("\\\""),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(q);
    out
}

/// Format a single complex component the way CPython's repr does:
///   - integer-valued floats with |v| < 1e16 → `"3"` (no `.0`)
///   - |v| >= 1e16 → scientific notation `"1e+20"` (Python style)
///   - NaN / inf via `format_float`
///   - everything else → standard float repr
///
/// Python uses scientific notation for absolute values >= 1e16 (where i64
/// rounding would lose precision) and for very small non-zero values; we
/// mirror that boundary.
fn complex_component(v: f64) -> String {
    if !v.is_finite() {
        return format_float(v);
    }
    let abs = v.abs();
    if v == v.trunc() && abs < 1e16 {
        // -0.0 as i64 yields 0, losing the sign.  Preserve it explicitly.
        if v == 0.0 && v.is_sign_negative() {
            return "-0".to_string();
        }
        return format!("{}", v as i64);
    }
    if abs >= 1e16 || (abs != 0.0 && abs < 1e-4) {
        // Rust's `{:e}` produces "1e20"; CPython prints "1e+20". Patch the sign.
        let raw = format!("{v:e}");
        if let Some(idx) = raw.find('e') {
            let (mantissa, exp) = raw.split_at(idx);
            let exp = &exp[1..]; // skip 'e'
            if let Some(stripped) = exp.strip_prefix('-') {
                return format!("{mantissa}e-{stripped:0>2}");
            }
            return format!("{mantissa}e+{exp:0>2}");
        }
        return raw;
    }
    format_float(v)
}

/// Format a complex number the way Python does:
///   `1j`, `(2+3j)`, `(2-3j)`, `(-1+0j)`, etc.
fn complex_repr(re: f64, im: f64) -> String {
    let im_str = complex_component(im);
    if re == 0.0 && (1.0_f64).copysign(re) > 0.0 {
        return format!("{im_str}j");
    }
    let re_str = complex_component(re);
    let sep = if im < 0.0 || (im == 0.0 && im.is_sign_negative()) {
        ""
    } else {
        "+"
    };
    format!("({re_str}{sep}{im_str}j)")
}

/// Canonical CPython-parity `repr()` for a `PyKey`.
///
/// This is the single source of truth for hashable-key reprs across the
/// workspace.  Consumers in `pyrust-builtins` (frozenset, dict views) and
/// `pyrust` (collections.deque) all route through this function rather than
/// keeping local copies — see issue #422 for the divergence that motivated
/// consolidation (whole-number floats were losing their trailing `.0` in the
/// frozenset path because the copy there used `f64::to_string` instead of
/// `format_float`).
///
/// Umbrella tracking: #420.
pub fn key_repr(key: &PyKey) -> String {
    match key {
        PyKey::Int(v) => v.to_string(),
        PyKey::BigInt(v) => v.to_string(),
        PyKey::Float(v) => format_float(f64::from_bits(*v)),
        PyKey::Str(v) => {
            let s = v.as_str().unwrap_or("");
            let q = repr_quote(s);
            format!("{}{}{}", q, escape_str(s, q), q)
        }
        PyKey::Bool(v) => {
            if *v {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        PyKey::None => "None".to_string(),
        PyKey::Ellipsis => "Ellipsis".to_string(),
        PyKey::FrozenSet(key) => {
            if key.items().is_empty() {
                "frozenset()".to_string()
            } else {
                let inner = key
                    .items()
                    .iter()
                    .map(key_repr)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("frozenset({{{inner}}})")
            }
        }
        PyKey::Tuple(items) => {
            if items.is_empty() {
                "()".to_string()
            } else if items.len() == 1 {
                format!("({},)", key_repr(&items[0]))
            } else {
                let inner = items.iter().map(key_repr).collect::<Vec<_>>().join(", ");
                format!("({inner})")
            }
        }
        PyKey::Bytes(b) => bytes_repr(b),
        PyKey::Complex(re, im) => complex_repr(*re, *im),
        PyKey::Object { value, .. } => value.repr_raw(),
    }
}
