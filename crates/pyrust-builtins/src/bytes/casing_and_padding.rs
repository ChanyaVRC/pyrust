fn bytes_title(bytes: &[u8]) -> Vec<u8> {
    // A byte is a word character if it is ASCII alphabetic.
    // Non-alphabetic bytes act as word separators.
    let mut out = Vec::with_capacity(bytes.len());
    let mut prev_was_alpha = false;
    for &b in bytes {
        if b.is_ascii_alphabetic() {
            if prev_was_alpha {
                out.push(b.to_ascii_lowercase());
            } else {
                out.push(b.to_ascii_uppercase());
            }
            prev_was_alpha = true;
        } else {
            out.push(b);
            prev_was_alpha = false;
        }
    }
    out
}

fn bytes_capitalize(bytes: &[u8]) -> Vec<u8> {
    // First byte upper-cased (if alphabetic), rest lower-cased.
    let mut out: Vec<u8> = bytes.iter().map(|b| b.to_ascii_lowercase()).collect();
    if let Some(first) = out.first_mut() {
        *first = first.to_ascii_uppercase();
    }
    out
}

// ---------------------------------------------------------------------------
// isupper / islower
// ---------------------------------------------------------------------------

fn bytes_isupper(bytes: &[u8]) -> bool {
    // Must have at least one uppercase letter and no lowercase letters.
    let has_upper = bytes.iter().any(|b| b.is_ascii_uppercase());
    let has_lower = bytes.iter().any(|b| b.is_ascii_lowercase());
    has_upper && !has_lower
}

fn bytes_islower(bytes: &[u8]) -> bool {
    // Must have at least one lowercase letter and no uppercase letters.
    let has_lower = bytes.iter().any(|b| b.is_ascii_lowercase());
    let has_upper = bytes.iter().any(|b| b.is_ascii_uppercase());
    has_lower && !has_upper
}

// ---------------------------------------------------------------------------
// center / ljust / rjust
// ---------------------------------------------------------------------------

fn extract_fill_byte(args: &[Value], arg_idx: usize, method: &str) -> Result<u8> {
    match args.get(arg_idx).map(|v| v.kind()) {
        None => Ok(b' '),
        Some(ValueKind::Bytes(rc)) => {
            let s = rc.as_slice();
            if s.len() == 1 {
                Ok(s[0])
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "{method} fillchar must be a byte string of length 1, not {}",
                        s.len()
                    ),
                ))
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!("{method} fillchar must be a bytes object of length 1"),
        )),
    }
}

fn extract_width(args: &[Value], method: &str) -> Result<i64> {
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => Ok(n),
        Some(ValueKind::Bool(b)) => Ok(b as i64),
        Some(ValueKind::BigInt(_)) => Err(PyError::named(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )),
        _ => Err(PyError::named(
            "TypeError",
            format!("{method} width must be an integer"),
        )),
    }
}

fn bytes_center(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let width = extract_width(args, "center")?;
    let fill = extract_fill_byte(args, 1, "center")?;
    let len = bytes.len();
    if width <= len as i64 {
        return Ok(Value::bytes(bytes.to_vec()));
    }
    let width = width as usize;
    let marg = width - len;
    // CPython formula: left = marg/2 + (marg & width & 1)
    let left = marg / 2 + (marg & width & 1);
    let right = marg - left;
    let mut out = Vec::with_capacity(width);
    out.extend(std::iter::repeat_n(fill, left));
    out.extend_from_slice(bytes);
    out.extend(std::iter::repeat_n(fill, right));
    Ok(Value::bytes(out))
}

fn bytes_ljust(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let width = extract_width(args, "ljust")?;
    let fill = extract_fill_byte(args, 1, "ljust")?;
    let len = bytes.len();
    if width <= len as i64 {
        return Ok(Value::bytes(bytes.to_vec()));
    }
    let width = width as usize;
    let mut out = Vec::with_capacity(width);
    out.extend_from_slice(bytes);
    out.extend(std::iter::repeat_n(fill, width - len));
    Ok(Value::bytes(out))
}

fn bytes_rjust(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let width = extract_width(args, "rjust")?;
    let fill = extract_fill_byte(args, 1, "rjust")?;
    let len = bytes.len();
    if width <= len as i64 {
        return Ok(Value::bytes(bytes.to_vec()));
    }
    let width = width as usize;
    let mut out = Vec::with_capacity(width);
    out.extend(std::iter::repeat_n(fill, width - len));
    out.extend_from_slice(bytes);
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// zfill
// ---------------------------------------------------------------------------

fn bytes_zfill(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let width: i64 = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "zfill width must be an integer".to_string(),
            ));
        }
    };
    let len = bytes.len();
    if width <= len as i64 {
        return Ok(Value::bytes(bytes.to_vec()));
    }
    let width = width as usize;
    let pad = width - len;
    let mut out = Vec::with_capacity(width);
    // If the first byte is '+' or '-', keep it at the front and pad after it.
    if len > 0 && (bytes[0] == b'+' || bytes[0] == b'-') {
        out.push(bytes[0]);
        out.extend(std::iter::repeat_n(b'0', pad));
        out.extend_from_slice(&bytes[1..]);
    } else {
        out.extend(std::iter::repeat_n(b'0', pad));
        out.extend_from_slice(bytes);
    }
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// translate
// ---------------------------------------------------------------------------
