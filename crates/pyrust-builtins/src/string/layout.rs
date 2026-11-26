// ─── Method implementations ───────────────────────────────────────────────────

/// Try to reserve `additional` bytes in `out`, mapping allocation failure to
/// `MemoryError` rather than panicking, mirroring CPython's behaviour.
#[inline]
fn try_reserve_str(out: &mut String, additional: usize) -> Result<()> {
    out.try_reserve(additional)
        .map_err(|_| PyError::named("MemoryError", ""))
}

/// Append `n` copies of the one-codepoint `fill` string.  The fill is kept
/// as a `&str` because a lone surrogate is a valid Python fill character but
/// not a Rust `char` (#3315).
#[inline]
fn push_fill(out: &mut String, fill: &str, n: usize) {
    match *fill.as_bytes() {
        [b] => out.extend(std::iter::repeat_n(char::from(b), n)),
        _ => {
            for _ in 0..n {
                out.push_str(fill);
            }
        }
    }
}

/// Python length of `s`: codepoints counted by lead byte, so a lone
/// surrogate counts as one and no `char` is decoded.
#[inline]
fn layout_len(s: &str, is_ascii: bool) -> usize {
    if is_ascii {
        s.len()
    } else {
        str_codepoint_count(s)
    }
}

fn str_center(src: &Value, s: &str, is_ascii: bool, width: i64, fill: &str) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = layout_len(s, is_ascii);
    if char_len >= width {
        return Ok(src.clone());
    }
    let marg = width - char_len;
    // CPython formula: left = marg//2 + (marg & width & 1)
    let left_pad = marg / 2 + (marg & width & 1);
    let right_pad = marg - left_pad;
    let fill_bytes = marg.saturating_mul(fill.len());
    let total = s.len().saturating_add(fill_bytes);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    push_fill(&mut out, fill, left_pad);
    out.push_str(s);
    push_fill(&mut out, fill, right_pad);
    Ok(Value::string(out))
}

fn str_ljust(src: &Value, s: &str, is_ascii: bool, width: i64, fill: &str) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = layout_len(s, is_ascii);
    if char_len >= width {
        return Ok(src.clone());
    }
    let pad = width - char_len;
    let fill_bytes = pad.saturating_mul(fill.len());
    let total = s.len().saturating_add(fill_bytes);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    out.push_str(s);
    push_fill(&mut out, fill, pad);
    Ok(Value::string(out))
}

fn str_rjust(src: &Value, s: &str, is_ascii: bool, width: i64, fill: &str) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = layout_len(s, is_ascii);
    if char_len >= width {
        return Ok(src.clone());
    }
    let pad = width - char_len;
    let fill_bytes = pad.saturating_mul(fill.len());
    let total = s.len().saturating_add(fill_bytes);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    push_fill(&mut out, fill, pad);
    out.push_str(s);
    Ok(Value::string(out))
}

fn str_zfill(src: &Value, s: &str, is_ascii: bool, width: i64) -> Result<Value> {
    let width = width.max(0) as usize;
    let char_len = layout_len(s, is_ascii);
    if char_len >= width {
        return Ok(src.clone());
    }
    let pad = width - char_len;
    let total = s.len().saturating_add(pad);
    let mut out = String::new();
    try_reserve_str(&mut out, total)?;
    // Preserve a leading sign character.  The sign test is a byte comparison,
    // so a leading lone surrogate is never decoded into a `char` (#3315).
    let rest = match s.as_bytes().first() {
        Some(&sign @ (b'+' | b'-')) => {
            out.push(char::from(sign));
            &s[1..]
        }
        _ => s,
    };
    for _ in 0..pad {
        out.push('0');
    }
    out.push_str(rest);
    Ok(Value::string(out))
}

fn str_expandtabs(src: &Value, s: &str, tabsize: i64) -> Value {
    if !s.as_bytes().contains(&b'\t') {
        return src.clone();
    }
    let tabsize = tabsize.max(0) as usize;
    let mut out = String::with_capacity(s.len());
    let mut col: usize = 0;
    if src.str_is_ascii() || !str_has_surrogates(s) {
        for c in s.chars() {
            match c {
                '\t' => push_tab_expansion(&mut out, &mut col, tabsize),
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
        return Value::string(out);
    }
    // Lone surrogates are not `char`s (decoding one is UB, #3315): scan
    // bytes instead.  Tabs and line breaks are ASCII (always codepoint
    // boundaries), every other codepoint advances the column once at its lead
    // byte, and the runs between tabs are copied as slices.
    let mut run_start = 0usize;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        match b {
            b'\t' => {
                out.push_str(&s[run_start..i]);
                run_start = i + 1;
                push_tab_expansion(&mut out, &mut col, tabsize);
            }
            b'\n' | b'\r' => col = 0,
            _ if b & 0xC0 != 0x80 => col += 1,
            _ => {}
        }
    }
    out.push_str(&s[run_start..]);
    Value::string(out)
}

/// Expand one tab at column `col`; a tab with size 0 is removed.
#[inline]
fn push_tab_expansion(out: &mut String, col: &mut usize, tabsize: usize) {
    if tabsize != 0 {
        let spaces = tabsize - (*col % tabsize);
        out.extend(std::iter::repeat_n(' ', spaces));
        *col += spaces;
    }
}
