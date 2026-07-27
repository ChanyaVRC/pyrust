fn str_partition(s: &str, sep: &str) -> Result<Value> {
    if sep.is_empty() {
        return Err(PyError::named("ValueError", "empty separator".to_string()));
    }
    let (before, found_sep, after) = match s.find(sep) {
        Some(pos) => (&s[..pos], sep, &s[pos + sep.len()..]),
        None => (s, "", ""),
    };
    Ok(Value::tuple(vec![
        Value::string(before),
        Value::string(found_sep),
        Value::string(after),
    ]))
}

fn str_rpartition(s: &str, sep: &str) -> Result<Value> {
    if sep.is_empty() {
        return Err(PyError::named("ValueError", "empty separator".to_string()));
    }
    let (before, found_sep, after) = match s.rfind(sep) {
        Some(pos) => (&s[..pos], sep, &s[pos + sep.len()..]),
        None => ("", "", s),
    };
    Ok(Value::tuple(vec![
        Value::string(before),
        Value::string(found_sep),
        Value::string(after),
    ]))
}

fn str_splitlines(s: &str, args: &[Value]) -> Result<Value> {
    let keepends =
        crate::method_signature::normalized_optional_bool("str", "splitlines", "keepends", args)?;
    let mut lines: Vec<Value> = Vec::new();
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut start = 0;
    let mut i = 0;
    while i < len {
        let b = bytes[i];
        // Detect line endings: \r\n, \r, \n, \x0b, \x0c, \x1c, \x1d, \x1e, \x85,  ,
        let eol_len: usize;
        let is_eol = match b {
            b'\n' | b'\x0b' | b'\x0c' | b'\x1c' | b'\x1d' | b'\x1e' => {
                eol_len = 1;
                true
            }
            b'\r' => {
                if i + 1 < len && bytes[i + 1] == b'\n' {
                    eol_len = 2;
                } else {
                    eol_len = 1;
                }
                true
            }
            0xC2 if i + 1 < len && bytes[i + 1] == 0x85 => {
                // U+0085 NEXT LINE encoded as UTF-8: 0xC2 0x85
                eol_len = 2;
                true
            }
            0xE2 if i + 2 < len
                && bytes[i + 1] == 0x80
                && (bytes[i + 2] == 0xA8 || bytes[i + 2] == 0xA9) =>
            {
                // U+2028 LINE SEPARATOR / U+2029 PARAGRAPH SEPARATOR: 0xE2 0x80 0xA8/0xA9
                eol_len = 3;
                true
            }
            _ => {
                eol_len = 0;
                false
            }
        };
        if is_eol {
            let end = if keepends { i + eol_len } else { i };
            lines.push(Value::string(&s[start..end]));
            i += eol_len;
            start = i;
        } else {
            i += 1;
        }
    }
    // Trailing non-empty segment (no trailing newline)
    if start < len {
        lines.push(Value::string(&s[start..]));
    }
    Ok(Value::list(lines))
}

fn str_removeprefix(src: &Value, s: &str, prefix: &str) -> Value {
    if prefix.is_empty() {
        src.clone()
    } else if s.starts_with(prefix) {
        src.string_slice(prefix.len(), s.len())
    } else {
        src.clone()
    }
}

fn str_removesuffix(src: &Value, s: &str, suffix: &str) -> Value {
    if suffix.is_empty() {
        src.clone()
    } else if s.ends_with(suffix) {
        src.string_slice(0, s.len() - suffix.len())
    } else {
        src.clone()
    }
}
