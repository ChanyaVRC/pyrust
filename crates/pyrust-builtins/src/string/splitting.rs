/// `str.split()` / `str.split(None, maxsplit)`: CPython's `split_whitespace`.
///
/// Runs of `Py_UNICODE_ISSPACE` characters separate the parts; once
/// `maxsplit` parts are taken, leading whitespace of the remainder is skipped
/// and the rest (trailing whitespace included) is the final part.  Scans raw
/// bytes (see [`py_space_len_at`]), so lone surrogates are ordinary
/// non-whitespace characters and nothing is decoded into a `char` (#3315).
fn split_on_whitespace(src: &Value, s: &str, maxsplit: i64) -> Vec<Value> {
    let b = s.as_bytes();
    let len = b.len();
    let mut remaining = usize::try_from(maxsplit).unwrap_or(usize::MAX);
    // Heuristic capacity (avg word ~4 chars) avoids Vec realloc in one pass.
    let mut parts = Vec::with_capacity(if maxsplit < 0 { len / 4 + 1 } else { 0 });
    let mut i = 0;
    while remaining > 0 {
        i = skip_py_spaces(b, i);
        if i == len {
            break;
        }
        let start = i;
        // Step from codepoint boundary to codepoint boundary.
        i = (i + codepoint_width(b[i])).min(len);
        while i < len && py_space_len_at(b, i) == 0 {
            i = (i + codepoint_width(b[i])).min(len);
        }
        parts.push(src.string_slice(start, i));
        remaining -= 1;
    }
    if i < len {
        i = skip_py_spaces(b, i);
        if i < len {
            parts.push(src.string_slice(i, len));
        }
    }
    parts
}

/// `str.rsplit(None, maxsplit)` with `maxsplit >= 0`: CPython's
/// `rsplit_whitespace`, the mirror image of [`split_on_whitespace`].
fn rsplit_on_whitespace(src: &Value, s: &str, maxsplit: usize) -> Vec<Value> {
    let b = s.as_bytes();
    let mut remaining = maxsplit;
    let mut parts = Vec::new();
    let mut i = b.len();
    while remaining > 0 {
        i = skip_py_spaces_back(b, i);
        if i == 0 {
            break;
        }
        let end = i;
        // Step back bytewise; a whitespace match always ends at a codepoint
        // boundary (see [`py_space_len_before`]).
        i -= 1;
        while i > 0 && py_space_len_before(b, i) == 0 {
            i -= 1;
        }
        parts.push(src.string_slice(i, end));
        remaining -= 1;
    }
    if i > 0 {
        i = skip_py_spaces_back(b, i);
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
