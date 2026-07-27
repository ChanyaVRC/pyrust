fn str_startswith(s: &str, is_ascii: bool, args: &[Value]) -> Result<Value> {
    // str_slice_args returns None for an inverted window (start > end).
    // For a Str prefix, that is an immediate False.
    // For a Tuple, CPython still validates element types even on an inverted
    // window — TypeError takes priority over the inverted-range short-circuit.
    let window = str_slice_args(s, is_ascii, args)?;
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(p)) => {
            let Some((start, end)) = window else {
                return Ok(Value::bool_(false));
            };
            Ok(Value::bool_(s[start..end].starts_with(p)))
        }
        Some(ValueKind::Tuple(prefixes)) => {
            for pv in prefixes.iter() {
                match pv.kind() {
                    ValueKind::Str(p) => {
                        if let Some((start, end)) = window
                            && s[start..end].starts_with(p)
                        {
                            return Ok(Value::bool_(true));
                        }
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "tuple for startswith must only contain str, not {}",
                                builtin_type_name(pv)
                            ),
                        ));
                    }
                }
            }
            Ok(Value::bool_(false))
        }
        Some(_) => Err(PyError::named(
            "TypeError",
            format!(
                "startswith first arg must be str or a tuple of str, not {}",
                builtin_type_name(args.first().unwrap())
            ),
        )),
        None => Err(PyError::named(
            "TypeError",
            "startswith() takes at least 1 argument (0 given)".to_string(),
        )),
    }
}

fn str_endswith(s: &str, is_ascii: bool, args: &[Value]) -> Result<Value> {
    // str_slice_args returns None for an inverted window (start > end).
    // For a Str suffix, that is an immediate False.
    // For a Tuple, CPython still validates element types even on an inverted
    // window — TypeError takes priority over the inverted-range short-circuit.
    let window = str_slice_args(s, is_ascii, args)?;
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(p)) => {
            let Some((start, end)) = window else {
                return Ok(Value::bool_(false));
            };
            Ok(Value::bool_(s[start..end].ends_with(p)))
        }
        Some(ValueKind::Tuple(suffixes)) => {
            for sv in suffixes.iter() {
                match sv.kind() {
                    ValueKind::Str(p) => {
                        if let Some((start, end)) = window
                            && s[start..end].ends_with(p)
                        {
                            return Ok(Value::bool_(true));
                        }
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "tuple for endswith must only contain str, not {}",
                                builtin_type_name(sv)
                            ),
                        ));
                    }
                }
            }
            Ok(Value::bool_(false))
        }
        Some(_) => Err(PyError::named(
            "TypeError",
            format!(
                "endswith first arg must be str or a tuple of str, not {}",
                builtin_type_name(args.first().unwrap())
            ),
        )),
        None => Err(PyError::named(
            "TypeError",
            "endswith() takes at least 1 argument (0 given)".to_string(),
        )),
    }
}
