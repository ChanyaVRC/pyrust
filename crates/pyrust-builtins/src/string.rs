use pyrust_core::{PyError, Result, Value, ValueKind};

pub fn call(method: &str, src: &Value, args: &[Value]) -> Result<Value> {
    let s: &str = src.as_str().unwrap();
    match method {
        // Common Sequence Operations (via char indexing)
        "index" => str_index(s, args),
        "count" => str_count(s, args),
        // Splitting / joining
        "split" => split(src, s, args),
        "rsplit" => rsplit(src, s, args),
        "join" => join(s, args),
        // Stripping
        "strip" => Ok(Value::string(strip_chars(s, args, true, true))),
        "lstrip" => Ok(Value::string(strip_chars(s, args, true, false))),
        "rstrip" => Ok(Value::string(strip_chars(s, args, false, true))),
        // Case
        "upper" => Ok(Value::string(s.to_uppercase())),
        "lower" => Ok(Value::string(s.to_lowercase())),
        "capitalize" => Ok(Value::string(capitalize(s))),
        // Searching
        "find" => str_find(s, args, false),
        "rfind" => str_rfind(s, args, false),
        "rindex" => str_rfind(s, args, true),
        // Replacement
        "replace" => str_replace(s, args),
        // Testing
        "startswith" => str_startswith(s, args),
        "endswith" => str_endswith(s, args),
        "isdigit" => Ok(Value::bool_(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))),
        "isalpha" => Ok(Value::bool_(!s.is_empty() && s.chars().all(|c| c.is_alphabetic()))),
        "isalnum" => Ok(Value::bool_(!s.is_empty() && s.chars().all(|c| c.is_alphanumeric()))),
        "isspace" => Ok(Value::bool_(!s.is_empty() && s.chars().all(|c| c.is_whitespace()))),
        _ => Err(PyError::Runtime(format!(
            "'str' object has no attribute '{method}'"
        ))),
    }
}

fn str_index(s: &str, args: &[Value]) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        _ => return Err(PyError::Runtime("str.index() requires a str argument".to_string())),
    };
    let (start, end) = str_slice_args(s, args)?;
    let haystack = &s[start..end];
    match haystack.find(sub) {
        Some(byte_pos) => {
            let char_pos = s[..start + byte_pos].chars().count();
            Ok(Value::int(char_pos as i64))
        }
        None => Err(PyError::Runtime("substring not found".to_string())),
    }
}

fn str_count(s: &str, args: &[Value]) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        _ => return Err(PyError::Runtime("str.count() requires a str argument".to_string())),
    };
    if sub.is_empty() {
        return Ok(Value::int((s.chars().count() + 1) as i64));
    }
    let (start, end) = str_slice_args(s, args)?;
    let haystack = &s[start..end];
    let n = haystack.match_indices(sub).count();
    Ok(Value::int(n as i64))
}

fn str_find(s: &str, args: &[Value], raise_on_miss: bool) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        _ => return Err(PyError::Runtime("str.find() requires a str argument".to_string())),
    };
    let (start, end) = str_slice_args(s, args)?;
    let haystack = &s[start..end];
    match haystack.find(sub) {
        Some(byte_pos) => {
            let char_pos = s[..start + byte_pos].chars().count();
            Ok(Value::int(char_pos as i64))
        }
        None => {
            if raise_on_miss {
                Err(PyError::Runtime("substring not found".to_string()))
            } else {
                Ok(Value::int(-1))
            }
        }
    }
}

fn str_rfind(s: &str, args: &[Value], raise_on_miss: bool) -> Result<Value> {
    let sub = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => sub,
        _ => return Err(PyError::Runtime("str.rfind() requires a str argument".to_string())),
    };
    let (start, end) = str_slice_args(s, args)?;
    let haystack = &s[start..end];
    match haystack.rfind(sub) {
        Some(byte_pos) => {
            let char_pos = s[..start + byte_pos].chars().count();
            Ok(Value::int(char_pos as i64))
        }
        None => {
            if raise_on_miss {
                Err(PyError::Runtime("substring not found".to_string()))
            } else {
                Ok(Value::int(-1))
            }
        }
    }
}

fn split(src: &Value, s: &str, args: &[Value]) -> Result<Value> {
    let (sep, maxsplit) = split_args(args)?;
    let parts: Vec<Value> = match sep {
        None => {
            if maxsplit < 0 {
                // Heuristic capacity (avg word ~4 chars) avoids Vec realloc in one pass
                let mut parts = Vec::with_capacity(s.len() / 4 + 1);
                for p in s.split_whitespace() {
                    let off = p.as_ptr() as usize - s.as_ptr() as usize;
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
                    if t.is_empty() { break; }
                    match t.find(char::is_whitespace) {
                        None => {
                            let off = t.as_ptr() as usize - s.as_ptr() as usize;
                            out.push(src.string_slice(off, off + t.len()));
                            remaining = "";
                            break;
                        }
                        Some(pos) => {
                            let off = t.as_ptr() as usize - s.as_ptr() as usize;
                            out.push(src.string_slice(off, off + pos));
                            remaining = &t[pos..];
                        }
                    }
                }
                let tail = remaining.trim_start();
                if !tail.is_empty() {
                    let off = tail.as_ptr() as usize - s.as_ptr() as usize;
                    out.push(src.string_slice(off, off + tail.len()));
                }
                return Ok(Value::list(out));
            }
        }
        Some(sep_str) => {
            if maxsplit < 0 {
                let cap = if sep_str.is_empty() { s.len() + 1 } else { s.len() / sep_str.len() + 1 };
                let mut parts = Vec::with_capacity(cap);
                for p in s.split(sep_str) {
                    let off = p.as_ptr() as usize - s.as_ptr() as usize;
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts
            } else {
                s.splitn(maxsplit as usize + 1, sep_str).map(|p| {
                    let off = p.as_ptr() as usize - s.as_ptr() as usize;
                    src.string_slice(off, off + p.len())
                }).collect()
            }
        }
    };
    Ok(Value::list(parts))
}

fn rsplit(src: &Value, s: &str, args: &[Value]) -> Result<Value> {
    let (sep, maxsplit) = split_args(args)?;
    let parts: Vec<Value> = match sep {
        None => {
            // For rsplit with no sep, reverse the whitespace split
            if maxsplit < 0 {
                let mut parts = Vec::with_capacity(s.len() / 4 + 1);
                for p in s.split_whitespace() {
                    let off = p.as_ptr() as usize - s.as_ptr() as usize;
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts
            } else {
                let n = maxsplit as usize;
                let mut out = Vec::new();
                let mut remaining = s;
                for _ in 0..n {
                    let t = remaining.trim_end();
                    if t.is_empty() { break; }
                    match t.rfind(char::is_whitespace) {
                        None => {
                            let off = t.as_ptr() as usize - s.as_ptr() as usize;
                            out.push(src.string_slice(off, off + t.len()));
                            remaining = "";
                            break;
                        }
                        Some(pos) => {
                            let off = t[pos+1..].as_ptr() as usize - s.as_ptr() as usize;
                            out.push(src.string_slice(off, off + t[pos+1..].len()));
                            remaining = &t[..pos];
                        }
                    }
                }
                let head = remaining.trim_end();
                if !head.is_empty() {
                    let off = head.as_ptr() as usize - s.as_ptr() as usize;
                    out.push(src.string_slice(off, off + head.len()));
                }
                out.reverse();
                return Ok(Value::list(out));
            }
        }
        Some(sep_str) => {
            if maxsplit < 0 {
                let cap = if sep_str.is_empty() { s.len() + 1 } else { s.len() / sep_str.len() + 1 };
                let mut parts = Vec::with_capacity(cap);
                for p in s.split(sep_str) {
                    let off = p.as_ptr() as usize - s.as_ptr() as usize;
                    parts.push(src.string_slice(off, off + p.len()));
                }
                parts.reverse();
                parts
            } else {
                let mut parts: Vec<Value> = s.rsplitn(maxsplit as usize + 1, sep_str).map(|p| {
                    let off = p.as_ptr() as usize - s.as_ptr() as usize;
                    src.string_slice(off, off + p.len())
                }).collect();
                parts.reverse();
                parts
            }
        }
    };
    Ok(Value::list(parts))
}

fn join(sep: &str, args: &[Value]) -> Result<Value> {
    let iterable = args.first().ok_or_else(|| {
        PyError::Runtime("str.join() requires 1 argument".to_string())
    })?;
    let parts: Vec<String> = match iterable.kind() {
        ValueKind::List(items) => items
            .iter()
            .map(|v| match v.kind() {
                ValueKind::Str(s) => Ok(s.to_string()),
                _ => Err(PyError::Runtime(
                    "sequence item must be str".to_string(),
                )),
            })
            .collect::<Result<_>>()?,
        ValueKind::Tuple(items) => items
            .iter()
            .map(|v| match v.kind() {
                ValueKind::Str(s) => Ok(s.to_string()),
                _ => Err(PyError::Runtime(
                    "sequence item must be str".to_string(),
                )),
            })
            .collect::<Result<_>>()?,
        ValueKind::Str(s) => s.chars().map(|c| Ok(c.to_string())).collect::<Result<_>>()?,
        _ => return Err(PyError::Runtime("str.join() argument must be iterable".to_string())),
    };
    Ok(Value::string(parts.join(sep)))
}

fn str_replace(s: &str, args: &[Value]) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::Runtime("str.replace() requires 2 arguments".to_string()));
    }
    let old: &str = match args[0].kind() {
        ValueKind::Str(s) => s,
        _ => return Err(PyError::Runtime("str.replace() argument 1 must be str".to_string())),
    };
    let new: &str = match args[1].kind() {
        ValueKind::Str(s) => s,
        _ => return Err(PyError::Runtime("str.replace() argument 2 must be str".to_string())),
    };
    let count = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        None => -1,
        _ => return Err(PyError::Runtime("str.replace() count must be int".to_string())),
    };
    if count < 0 {
        Ok(Value::string(s.replace(old, new)))
    } else {
        Ok(Value::string(s.replacen(old, new, count as usize)))
    }
}

fn str_startswith(s: &str, args: &[Value]) -> Result<Value> {
    let prefix = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(p)) => p,
        _ => return Err(PyError::Runtime("str.startswith() requires a str argument".to_string())),
    };
    let (start, end) = str_slice_args(s, args)?;
    Ok(Value::bool_(s[start..end].starts_with(prefix)))
}

fn str_endswith(s: &str, args: &[Value]) -> Result<Value> {
    let suffix = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(p)) => p,
        _ => return Err(PyError::Runtime("str.endswith() requires a str argument".to_string())),
    };
    let (start, end) = str_slice_args(s, args)?;
    Ok(Value::bool_(s[start..end].ends_with(suffix)))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().to_string() + &c.as_str().to_lowercase(),
    }
}

fn strip_chars(s: &str, args: &[Value], left: bool, right: bool) -> String {
    let chars_arg: Option<&str> = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(c)) => Some(c),
        Some(ValueKind::None) | None => None,
        _ => None,
    };
    match chars_arg {
        None => {
            let mut result = s;
            if left { result = result.trim_start(); }
            if right { result = result.trim_end(); }
            result.to_string()
        }
        Some(chars) => {
            let chars_slice: Vec<char> = chars.chars().collect();
            let mut result = s;
            if left { result = result.trim_start_matches(|c: char| chars_slice.contains(&c)); }
            if right { result = result.trim_end_matches(|c: char| chars_slice.contains(&c)); }
            result.to_string()
        }
    }
}

/// Parse (sep, maxsplit) from split/rsplit args.
fn split_args<'a>(args: &'a [Value]) -> Result<(Option<&'a str>, i64)> {
    let sep = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(s)) => Some(s),
        Some(ValueKind::None) | None => None,
        _ => return Err(PyError::Runtime("split() separator must be str or None".to_string())),
    };
    let maxsplit = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        None => -1,
        _ => return Err(PyError::Runtime("split() maxsplit must be int".to_string())),
    };
    Ok((sep, maxsplit))
}

/// Convert char-based start/end args (args[1], args[2]) to byte offsets.
fn str_slice_args(s: &str, args: &[Value]) -> Result<(usize, usize)> {
    let char_len = s.chars().count();
    let start_char = match args.get(1).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_char_idx(i, char_len),
        Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, char_len),
        None => 0,
        _ => return Err(PyError::Runtime("slice indices must be integers".to_string())),
    };
    let end_char = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => normalise_char_idx(i, char_len).min(char_len),
        Some(ValueKind::Bool(b)) => normalise_char_idx(b as i64, char_len).min(char_len),
        None => char_len,
        _ => return Err(PyError::Runtime("slice indices must be integers".to_string())),
    };
    // Convert char indices to byte indices
    let start_byte = s.char_indices().nth(start_char).map(|(b, _)| b).unwrap_or(s.len());
    let end_byte = s.char_indices().nth(end_char).map(|(b, _)| b).unwrap_or(s.len());
    Ok((start_byte, end_byte))
}

fn normalise_char_idx(idx: i64, len: usize) -> usize {
    if idx < 0 {
        let from_end = (-idx) as usize;
        if from_end > len { 0 } else { len - from_end }
    } else {
        idx as usize
    }
}
