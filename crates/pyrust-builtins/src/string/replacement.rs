fn str_replace(src: &Value, s: &str, args: &[Value]) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            format!("replace expected at least 2 arguments, got {}", args.len()),
        ));
    }
    let old: &str = match args[0].kind() {
        ValueKind::Str(s) => s,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "replace() argument 1 must be str, not {}",
                    py_value_display_name(&args[0])
                ),
            ));
        }
    };
    let new: &str = match args[1].kind() {
        ValueKind::Str(s) => s,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "replace() argument 2 must be str, not {}",
                    py_value_display_name(&args[1])
                ),
            ));
        }
    };
    let count = match args.get(2).map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        None => -1,
        Some(ValueKind::BigInt(_)) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            ));
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    builtin_type_name(&args[2])
                ),
            ));
        }
    };
    let max = if count < 0 {
        usize::MAX
    } else {
        count as usize
    };
    if old == new {
        return Ok(src.clone());
    }
    Ok(match replace_fill(s, old, new, max) {
        Some(result) => Value::string(result),
        None => src.clone(),
    })
}

/// Single-pass `str.replace`/`replacen` that seeds the result buffer with
/// `s.len()` capacity.  Rust's `str::replace` starts from an empty `String` and
/// reallocates as it grows (several allocation events per call); most replaces
/// keep the length close to the source, so one up-front reservation avoids those
/// intermediate reallocations without the extra counting pass a *precise* size
/// would need.  Semantics are identical to `s.replacen(from, to, max)`
/// (`max == usize::MAX` for replace-all), including empty-`from` behaviour.
fn replace_fill(s: &str, from: &str, to: &str, max: usize) -> Option<String> {
    let mut matches = s.match_indices(from).take(max);
    let (first_start, first_part) = matches.next()?;
    let mut result = String::with_capacity(s.len());
    result.push_str(&s[..first_start]);
    result.push_str(to);
    let mut last_end = first_start + first_part.len();
    for (start, part) in matches {
        result.push_str(&s[last_end..start]);
        result.push_str(to);
        last_end = start + part.len();
    }
    result.push_str(&s[last_end..]);
    Some(result)
}
