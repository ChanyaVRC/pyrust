/// Decode the codepoint starting at byte `i` (ASCII in one comparison).
#[inline(always)]
fn next_cp_at(s: &str, i: usize) -> Option<(u32, usize)> {
    match s.as_bytes().get(i) {
        Some(&b) if b < 0x80 => Some((u32::from(b), i + 1)),
        Some(_) => cesu8_next_codepoint(s, i),
        None => None,
    }
}

/// Decode the codepoint ending at byte `i` (ASCII in one comparison).
#[inline(always)]
fn prev_cp_at(s: &str, i: usize) -> Option<(u32, usize)> {
    match i.checked_sub(1).map(|p| s.as_bytes()[p]) {
        Some(b) if b < 0x80 => Some((u32::from(b), i - 1)),
        Some(_) => cesu8_prev_codepoint(s, i),
        None => None,
    }
}

/// `str.split()` / `str.split(None, maxsplit)`: CPython's `split_whitespace`.
///
/// Runs of `Py_UNICODE_ISSPACE` characters separate the parts; once
/// `maxsplit` parts are taken, leading whitespace of the remainder is skipped
/// and the rest (trailing whitespace included) is the final part.  Works on
/// codepoints decoded with the surrogate-safe cursors, so lone surrogates are
/// ordinary non-whitespace characters (#3315).
fn split_on_whitespace(src: &Value, s: &str, maxsplit: i64) -> Vec<Value> {
    let len = s.len();
    let mut remaining = usize::try_from(maxsplit).unwrap_or(usize::MAX);
    // Heuristic capacity (avg word ~4 chars) avoids Vec realloc in one pass.
    let mut parts = Vec::with_capacity(if maxsplit < 0 { len / 4 + 1 } else { 0 });
    let mut i = 0;
    while remaining > 0 {
        while let Some((cp, next)) = next_cp_at(s, i) {
            if !is_python_space_cp(cp) {
                break;
            }
            i = next;
        }
        if i == len {
            break;
        }
        let start = i;
        while let Some((cp, next)) = next_cp_at(s, i) {
            if is_python_space_cp(cp) {
                break;
            }
            i = next;
        }
        parts.push(src.string_slice(start, i));
        remaining -= 1;
    }
    if i < len {
        while let Some((cp, next)) = next_cp_at(s, i) {
            if !is_python_space_cp(cp) {
                break;
            }
            i = next;
        }
        if i < len {
            parts.push(src.string_slice(i, len));
        }
    }
    parts
}

/// `str.rsplit(None, maxsplit)` with `maxsplit >= 0`: CPython's
/// `rsplit_whitespace`, the mirror image of [`split_on_whitespace`].
fn rsplit_on_whitespace(src: &Value, s: &str, maxsplit: usize) -> Vec<Value> {
    let mut remaining = maxsplit;
    let mut parts = Vec::new();
    let mut i = s.len();
    while remaining > 0 {
        while let Some((cp, prev)) = prev_cp_at(s, i) {
            if !is_python_space_cp(cp) {
                break;
            }
            i = prev;
        }
        if i == 0 {
            break;
        }
        let end = i;
        while let Some((cp, prev)) = prev_cp_at(s, i) {
            if is_python_space_cp(cp) {
                break;
            }
            i = prev;
        }
        parts.push(src.string_slice(i, end));
        remaining -= 1;
    }
    if i > 0 {
        while let Some((cp, prev)) = prev_cp_at(s, i) {
            if !is_python_space_cp(cp) {
                break;
            }
            i = prev;
        }
        if i > 0 {
            parts.push(src.string_slice(0, i));
        }
    }
    parts.reverse();
    parts
}

fn split(src: &Value, s: &str, args: &[Value]) -> Result<Value> {
    let (sep, maxsplit) = split_args(args)?;
    let parts: Vec<Value> = match sep {
        None => split_on_whitespace(src, s, maxsplit),
        Some(sep_str) => {
            if sep_str.is_empty() {
                return Err(PyError::named("ValueError", "empty separator".to_string()));
            }
            if maxsplit < 0 {
                let cap = s.len() / sep_str.len() + 1;
                let mut parts = Vec::with_capacity(cap);
                for p in s.split(sep_str) {
                    let off = subslice_offset(s, p);
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts
            } else {
                s.splitn(maxsplit as usize + 1, sep_str)
                    .map(|p| {
                        let off = subslice_offset(s, p);
                        src.string_slice(off, off + p.len())
                    })
                    .collect()
            }
        }
    };
    Ok(Value::list(parts))
}

fn rsplit(src: &Value, s: &str, args: &[Value]) -> Result<Value> {
    let (sep, maxsplit) = split_args(args)?;
    let parts: Vec<Value> = match sep {
        // Without a limit, splitting from the right yields the same parts.
        None if maxsplit < 0 => split_on_whitespace(src, s, maxsplit),
        None => rsplit_on_whitespace(src, s, maxsplit as usize),
        Some(sep_str) => {
            if sep_str.is_empty() {
                return Err(PyError::named("ValueError", "empty separator".to_string()));
            }
            if maxsplit < 0 {
                let cap = s.len() / sep_str.len() + 1;
                let mut parts = Vec::with_capacity(cap);
                for p in s.split(sep_str) {
                    let off = subslice_offset(s, p);
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts
            } else {
                let mut parts: Vec<Value> = s
                    .rsplitn(maxsplit as usize + 1, sep_str)
                    .map(|p| {
                        let off = subslice_offset(s, p);
                        src.string_slice(off, off + p.len())
                    })
                    .collect();
                parts.reverse();
                parts
            }
        }
    };
    Ok(Value::list(parts))
}
