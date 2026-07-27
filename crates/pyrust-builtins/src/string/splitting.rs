fn split(src: &Value, s: &str, args: &[Value]) -> Result<Value> {
    let (sep, maxsplit) = split_args(args)?;
    let parts: Vec<Value> = match sep {
        None => {
            if maxsplit < 0 {
                // Heuristic capacity (avg word ~4 chars) avoids Vec realloc in one pass
                let mut parts = Vec::with_capacity(s.len() / 4 + 1);
                for p in s.split_whitespace() {
                    let off = subslice_offset(s, p);
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts
            } else {
                let n = maxsplit as usize;
                // Python's whitespace split: consecutive whitespace treated as one
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    return Ok(Value::list(vec![]));
                }
                let mut out = Vec::new();
                let mut remaining = s;
                for _ in 0..n {
                    let t = remaining.trim_start();
                    if t.is_empty() {
                        break;
                    }
                    match t.find(char::is_whitespace) {
                        None => {
                            let off = subslice_offset(s, t);
                            out.push(src.string_slice(off, off + t.len()));
                            remaining = "";
                            break;
                        }
                        Some(pos) => {
                            let off = subslice_offset(s, t);
                            out.push(src.string_slice(off, off + pos));
                            remaining = &t[pos..];
                        }
                    }
                }
                let tail = remaining.trim_start();
                if !tail.is_empty() {
                    let off = subslice_offset(s, tail);
                    out.push(src.string_slice(off, off + tail.len()));
                }
                return Ok(Value::list(out));
            }
        }
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
        None => {
            // No sep: rsplit with no maxsplit is identical to split (left-to-right).
            if maxsplit < 0 {
                let mut parts = Vec::with_capacity(s.len() / 4 + 1);
                for p in s.split_whitespace() {
                    let off = subslice_offset(s, p);
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts
            } else {
                let n = maxsplit as usize;
                let mut out = Vec::new();
                let mut remaining = s;
                for _ in 0..n {
                    let t = remaining.trim_end();
                    if t.is_empty() {
                        break;
                    }
                    match t.rfind(char::is_whitespace) {
                        None => {
                            let off = subslice_offset(s, t);
                            out.push(src.string_slice(off, off + t.len()));
                            remaining = "";
                            break;
                        }
                        Some(pos) => {
                            let tail = &t[pos + 1..];
                            let off = subslice_offset(s, tail);
                            out.push(src.string_slice(off, off + tail.len()));
                            remaining = &t[..pos];
                        }
                    }
                }
                let head = remaining.trim_end();
                if !head.is_empty() {
                    let off = subslice_offset(s, head);
                    out.push(src.string_slice(off, off + head.len()));
                }
                out.reverse();
                return Ok(Value::list(out));
            }
        }
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
