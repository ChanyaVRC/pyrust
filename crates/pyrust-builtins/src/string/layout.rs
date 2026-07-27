// ─── Method implementations ───────────────────────────────────────────────────

/// Try to reserve `additional` bytes in `out`, mapping allocation failure to
/// `MemoryError` rather than panicking, mirroring CPython's behaviour.
#[inline]
fn try_reserve_str(out: &mut String, additional: usize) -> Result<()> {
    out.try_reserve(additional)
        .map_err(|_| PyError::named("MemoryError", ""))
}

fn str_center(src: &Value, s: &str, is_ascii: bool, width: i64, fill: char) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = if is_ascii { s.len() } else { s.chars().count() };
    if char_len >= width {
        return Ok(src.clone());
    }
    let marg = width - char_len;
    // CPython formula: left = marg//2 + (marg & width & 1)
    let left_pad = marg / 2 + (marg & width & 1);
    let right_pad = marg - left_pad;
    let fill_bytes = marg.saturating_mul(fill.len_utf8());
    let total = s.len().saturating_add(fill_bytes);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    for _ in 0..left_pad {
        out.push(fill);
    }
    out.push_str(s);
    for _ in 0..right_pad {
        out.push(fill);
    }
    Ok(Value::string(out))
}

fn str_ljust(src: &Value, s: &str, is_ascii: bool, width: i64, fill: char) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = if is_ascii { s.len() } else { s.chars().count() };
    if char_len >= width {
        return Ok(src.clone());
    }
    let pad = width - char_len;
    let fill_bytes = pad.saturating_mul(fill.len_utf8());
    let total = s.len().saturating_add(fill_bytes);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    out.push_str(s);
    for _ in 0..pad {
        out.push(fill);
    }
    Ok(Value::string(out))
}

fn str_rjust(src: &Value, s: &str, is_ascii: bool, width: i64, fill: char) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = if is_ascii { s.len() } else { s.chars().count() };
    if char_len >= width {
        return Ok(src.clone());
    }
    let pad = width - char_len;
    let fill_bytes = pad.saturating_mul(fill.len_utf8());
    let total = s.len().saturating_add(fill_bytes);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    for _ in 0..pad {
        out.push(fill);
    }
    out.push_str(s);
    Ok(Value::string(out))
}

fn str_zfill(src: &Value, s: &str, is_ascii: bool, width: i64) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = if is_ascii { s.len() } else { s.chars().count() };
    if char_len >= width {
        return Ok(src.clone());
    }
    let pad = width - char_len;
    let total = s.len().saturating_add(pad);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    // Preserve leading sign character
    let mut chars = s.chars();
    match chars.next() {
        Some(c @ ('+' | '-')) => {
            out.push(c);
            for _ in 0..pad {
                out.push('0');
            }
            out.push_str(chars.as_str());
        }
        Some(first) => {
            for _ in 0..pad {
                out.push('0');
            }
            out.push(first);
            out.push_str(chars.as_str());
        }
        None => {
            for _ in 0..pad {
                out.push('0');
            }
        }
    }
    Ok(Value::string(out))
}

fn str_expandtabs(src: &Value, s: &str, tabsize: i64) -> Value {
    if !s.as_bytes().contains(&b'\t') {
        return src.clone();
    }
    let tabsize = tabsize.max(0) as usize;
    let mut out = String::with_capacity(s.len());
    let mut col: usize = 0;
    for c in s.chars() {
        match c {
            '\t' => {
                if tabsize == 0 {
                    // tab with size 0 is removed
                } else {
                    let spaces = tabsize - (col % tabsize);
                    for _ in 0..spaces {
                        out.push(' ');
                    }
                    col += spaces;
                }
            }
            '\n' | '\r' => {
                out.push(c);
                col = 0;
            }
            _ => {
                out.push(c);
                col += 1;
            }
        }
    }
    Value::string(out)
}
