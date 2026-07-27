/// Apply the parsed spec to the value.  Splits into a string-typed branch
/// and a numeric branch so type-specific validation stays close to formatting.
fn render_format_spec(value: &Value, fs: &FormatSpec, type_name: &str) -> Result<String> {
    // Treat the value as a string when the type code is 's' (or absent and
    // the value is a string).  For non-string values with no type code, fall
    // back to numeric handling so width / zero-pad / sign still apply.
    let is_string_target = matches!(fs.type_char, Some('s'))
        || (fs.type_char.is_none() && matches!(value.kind(), ValueKind::Str(_)));

    if is_string_target {
        return format_as_string(value, fs, type_name);
    }

    // No type code and a non-string value: route by value kind.
    if fs.type_char.is_none() {
        match value.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
                return format_int_value(value, fs, None, type_name);
            }
            ValueKind::Float(_) => return format_float_value(value, fs, None, type_name),
            // Complex with no explicit type code: render via complex_repr
            // (matching CPython's `format(1+2j)` -> "(1+2j)") and then apply
            // width / align / fill to the resulting string.  The float
            // format codes are rejected for Complex in format_float_value,
            // so we must short-circuit here for the bare-spec case.
            ValueKind::Complex(_, _) => return format_complex_value(value, fs),
            _ => {
                // Anything else: fall back to str() then pad like a string.
                return format_as_string(value, fs, type_name);
            }
        }
    }

    let t = fs.type_char.unwrap();
    // Complex supports the float presentation types f/F/e/E/g/G plus 'n'
    // (locale-as-'g'), applying the spec to both components.  It does NOT
    // support '%' or any integer/string code.
    if matches!(value.kind(), ValueKind::Complex(_, _)) {
        if matches!(t, 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | 'n') {
            return format_complex_value(value, fs);
        }
        return Err(pyrust_core::value_err!(
            "Unknown format code '{}' for object of type '{type_name}'",
            format_code_repr(t)
        ));
    }
    match t {
        // 'n' is locale-aware and supported on both integer and float values.
        // Route it by the value's type: int/bool to the integer formatter,
        // float to the float formatter (which treats 'n' as 'g' since pyrust
        // has no locale, matching CPython's C-locale behavior where n == g).
        'n' if matches!(value.kind(), ValueKind::Float(_)) => {
            format_float_value(value, fs, Some('n'), type_name)
        }
        'd' | 'b' | 'o' | 'x' | 'X' | 'c' | 'n' => format_int_value(value, fs, Some(t), type_name),
        'e' | 'E' | 'f' | 'F' | 'g' | 'G' | '%' => {
            format_float_value(value, fs, Some(t), type_name)
        }
        's' => format_as_string(value, fs, type_name),
        _ => Err(pyrust_core::value_err!(
            "Unknown format code '{}' for object of type '{type_name}'",
            format_code_repr(t)
        )),
    }
}

fn format_as_string(value: &Value, fs: &FormatSpec, type_name: &str) -> Result<String> {
    // Reject numeric-only options on strings, matching CPython.
    if matches!(fs.type_char, Some('s')) && !matches!(value.kind(), ValueKind::Str(_)) {
        return Err(pyrust_core::value_err!(
            "Unknown format code 's' for object of type '{type_name}'"
        ));
    }
    if fs.sign.is_some() {
        return Err(pyrust_core::value_err!(
            "Sign not allowed in string format specifier"
        ));
    }
    if fs.alt {
        return Err(pyrust_core::value_err!(
            "Alternate form (#) not allowed in string format specifier"
        ));
    }
    if let Some(g) = fs.grouping {
        // CPython names the *actual* separator that was supplied, e.g.
        // "Cannot specify ',' with 's'." / "Cannot specify '_' with 's'."
        return Err(pyrust_core::value_err!("Cannot specify '{g}' with 's'."));
    }
    if matches!(fs.align, Some('=')) {
        return Err(pyrust_core::value_err!(
            "'=' alignment not allowed in string format specifier"
        ));
    }

    let raw = match value.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => value.to_py_str(),
    };
    let raw = match fs.precision {
        Some(p) => raw.chars().take(p).collect::<String>(),
        None => raw,
    };
    // CPython accepts the `0` zero-pad flag on strings (with a
    // DeprecationWarning) — it promotes the fill character to '0' but keeps
    // the default left-alignment.  We just match the run-time behavior.
    let effective_fill = if fs.zero_pad && !fs.fill_explicit {
        '0'
    } else {
        fs.fill
    };
    Ok(pad_value(&raw, fs, '<', effective_fill))
}
