fn bytes_decode(bytes: &[u8], args: &[Value], kwargs: &PyDict) -> Result<Value> {
    // Signature: decode(encoding='utf-8', errors='strict')
    //
    // CPython checks the total argument count before individual duplicate checks.
    // When positional args are present the message says "arguments"; when it is
    // all-kwargs it says "keyword arguments".
    let total = args.len() + kwargs.len();
    if total > 2 {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("decode() takes at most 2 keyword arguments ({total} given)"),
            ));
        }
        return Err(PyError::named(
            "TypeError",
            format!("decode() takes at most 2 arguments ({total} given)"),
        ));
    }
    // Reject unknown keyword arguments first.
    for key in kwargs.keys() {
        if let PyKey::Str(s) = key {
            let name = s.as_str().unwrap_or("");
            if name != "encoding" && name != "errors" {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{name}' is an invalid keyword argument for decode()"),
                ));
            }
        }
    }

    let kw_encoding: Option<String> = match kwargs.get(&StrKey("encoding")) {
        None => None,
        Some(v) => match v.kind() {
            ValueKind::Str(s) => Some(s.to_owned()),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "decode() argument 'encoding' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };
    let kw_errors: Option<String> = match kwargs.get(&StrKey("errors")) {
        None => None,
        Some(v) => match v.kind() {
            ValueKind::Str(s) => Some(s.to_owned()),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "decode() argument 'errors' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };

    // Validate that a keyword isn't also supplied positionally.
    if !args.is_empty() && kw_encoding.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for decode() given by name ('encoding') and position (1)".to_string(),
        ));
    }
    if args.get(1).is_some() && kw_errors.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for decode() given by name ('errors') and position (2)".to_string(),
        ));
    }

    let encoding: &str = match args.first() {
        None => kw_encoding.as_deref().unwrap_or("utf-8"),
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "decode() argument 'encoding' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };

    let errors: &str = match args.get(1) {
        None => kw_errors.as_deref().unwrap_or("strict"),
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "decode() argument 'errors' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };

    decode_bytes(bytes, encoding, errors)
}
