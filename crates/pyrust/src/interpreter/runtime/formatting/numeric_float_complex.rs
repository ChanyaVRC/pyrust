fn format_float_value(
    value: &Value,
    fs: &FormatSpec,
    type_char: Option<char>,
    type_name: &str,
) -> Result<String> {
    // Complex numbers don't yet support the explicit float / int type codes
    // here.  The bare-spec (no type char) path routes Complex through
    // `format_complex_value` before reaching this function, so a Complex
    // value here means the user supplied an unsupported type code.
    if matches!(value.kind(), ValueKind::Complex(_, _)) {
        let code = type_char.unwrap_or('\0');
        return Err(pyrust_core::value_err!(
            "Unknown format code '{}' for object of type '{type_name}'",
            format_code_repr(code)
        ));
    }

    // str.__format__ rejects float format codes with ValueError (matching
    // CPython's "Unknown format code 'f' for object of type 'str'").  The
    // generic `fmt_value_to_float` would raise TypeError instead, so we
    // intercept str values here before the conversion attempt.
    if matches!(value.kind(), ValueKind::Str(_)) {
        let code = type_char.unwrap_or('\0');
        return Err(pyrust_core::value_err!(
            "Unknown format code '{}' for object of type '{type_name}'",
            format_code_repr(code)
        ));
    }

    let f = fmt_value_to_float(value)?;
    let t = type_char.unwrap_or('\0'); // '\0' = no type, use shortest repr-ish

    let negative = f.is_sign_negative() && !f.is_nan();
    let sign_prefix = sign_prefix_for(negative, fs.sign);
    let abs_f = f.abs();

    // Special values: inf / nan ignore precision / alt / grouping.
    if f.is_nan() {
        let mut body = if matches!(t, 'F' | 'G' | 'E') {
            "NAN".to_string()
        } else {
            "nan".to_string()
        };
        // The '%' presentation type still appends the percent sign for non-finite
        // values: format(nan, '%') -> 'nan%' (#2027).
        if t == '%' {
            body.push('%');
        }
        // nan has no sign, but the explicit sign flag ('+' / ' ') still applies
        // (CPython: format(nan, '+') -> '+nan').  Grouping is ignored for
        // non-finite values: the zero-fill must be a solid block, not
        // comma-grouped synthetic digits (#2504).
        let fs = FormatSpec {
            grouping: None,
            ..fs.clone()
        };
        return Ok(assemble_numeric(sign_prefix, "", body, &fs, '>', 3));
    }
    if f.is_infinite() {
        let mut body = if matches!(t, 'F' | 'G' | 'E') {
            "INF".to_string()
        } else {
            "inf".to_string()
        };
        // '%' still appends the percent sign: format(inf, '%') -> 'inf%' (#2027).
        if t == '%' {
            body.push('%');
        }
        // Grouping is ignored for non-finite values: the zero-fill must be a
        // solid block, not comma-grouped synthetic digits (#2504).
        let fs = FormatSpec {
            grouping: None,
            ..fs.clone()
        };
        return Ok(assemble_numeric(sign_prefix, "", body, &fs, '>', 3));
    }

    // Validate grouping vs type.  Comma and '_' are allowed on all float
    // types except 'n', which already implies locale-aware grouping and so
    // CPython rejects an explicit ',' / '_' combined with it.
    if let Some(g) = fs.grouping
        && t == 'n'
    {
        return Err(pyrust_core::value_err!("Cannot specify '{g}' with 'n'."));
    }

    let (mut body, alt_prefix) = match t {
        'f' | 'F' => {
            let prec = fs.precision.unwrap_or(6);
            let s = format!("{:.prec$}", abs_f);
            let s = if t == 'F' { s.to_uppercase() } else { s };
            (ensure_alt_float(s, fs.alt, fs.precision), "")
        }
        'e' | 'E' => {
            let prec = fs.precision.unwrap_or(6);
            let s = if t == 'E' {
                format!("{:.prec$E}", abs_f)
            } else {
                format!("{:.prec$e}", abs_f)
            };
            let s = normalise_exp_digits(s);
            (ensure_alt_float(s, fs.alt, fs.precision), "")
        }
        // 'g'/'G' general format and 'n' (locale-aware general format). In
        // pyrust's locale-free C-locale behavior, 'n' is identical to 'g':
        // same default precision, same trailing-zero stripping, same exponent
        // threshold, and lowercase output (no uppercase 'N' variant exists).
        'g' | 'G' | 'n' => {
            let upper = t == 'G';
            let prec = fs.precision.unwrap_or(6);
            let prec = if prec == 0 { 1 } else { prec };
            // `format_g` keeps trailing zeros and the decimal point when the
            // alternate '#' form is requested (#1950).
            let s = format_g(abs_f, prec, upper, fs.alt);
            (s, "")
        }
        '%' => {
            let prec = fs.precision.unwrap_or(6);
            let s = format!("{:.prec$}", abs_f * 100.0);
            let s = ensure_alt_float(s, fs.alt, fs.precision);
            (format!("{s}%"), "")
        }
        _ => {
            // No explicit type char.  When precision is given, CPython's
            // no-type-char float format differs from 'g' in two ways:
            //
            //  1. Exponential threshold: use `exp >= max(prec - 1, 0)` (one
            //     step earlier than 'g' which uses `exp >= prec`).
            //  2. Fixed notation must preserve at least one decimal digit
            //     (e.g. `10.0` not `10`).
            //
            // Without precision, use a shortest-roundtrip-ish repr with at
            // least one digit after the decimal point.
            let s = if let Some(prec) = fs.precision {
                format_no_type_with_prec(abs_f, prec)
            } else {
                match value.kind() {
                    ValueKind::Float(_) => {
                        let raw = Value::float(abs_f).to_py_str();
                        if !raw.contains('.') && !raw.contains('e') && !raw.contains('n') {
                            format!("{raw}.0")
                        } else {
                            raw
                        }
                    }
                    ValueKind::Int(n) => {
                        let n = if n < 0 { -n } else { n };
                        format!("{n}.0")
                    }
                    ValueKind::Bool(b) => if b { "1.0" } else { "0.0" }.to_string(),
                    _ => format!("{abs_f}"),
                }
            };
            (s, "")
        }
    };

    // Apply grouping on the integer part of the float body.
    if let Some(g) = fs.grouping
        && (g == ',' || g == '_')
    {
        body = group_float_int_part(&body, g);
    }

    Ok(assemble_numeric(sign_prefix, alt_prefix, body, fs, '>', 3))
}

/// Format a Complex value.
///
/// CPython applies the float format mini-language to both the real and the
/// imaginary part, joining them as `<re><signed-im>j`.  The imaginary part
/// always carries an explicit sign; the real part follows the spec's sign
/// flag.  Width / fill / alignment then apply to the assembled string.
///
/// When no presentation type code is given, the components use the repr-style
/// float format (with the spec's precision, if any) and the result is wrapped
/// in parentheses unless the real part is positive zero — matching CPython's
/// `format(1+2j)` -> `"(1+2j)"` and `format(2j)` -> `"2j"`.  A presentation
/// type code (f/F/e/E/g/G/n) suppresses the parentheses.
///
/// Zero-padding and `=` alignment are always rejected for complex (the `j`
/// suffix / optional parens make interior padding ill-defined).
fn format_complex_value(value: &Value, fs: &FormatSpec) -> Result<String> {
    if fs.zero_pad && !fs.fill_explicit {
        return Err(pyrust_core::value_err!(
            "Zero padding is not allowed in complex format specifier"
        ));
    }
    if matches!(fs.align, Some('=')) {
        return Err(pyrust_core::value_err!(
            "'=' alignment flag is not allowed in complex format specifier"
        ));
    }

    let (re, im) = match value.kind() {
        ValueKind::Complex(re, im) => (re, im),
        // Non-complex values never reach this routine.
        _ => unreachable!("format_complex_value called on non-complex value"),
    };

    // The complex 'n' type maps to 'g' for the components (no locale grouping
    // here).  With no explicit type CPython uses a repr-style component: when
    // a precision is supplied it behaves like 'g' with that precision;
    // otherwise it is the shortest round-trip repr with integer-valued floats
    // rendered without the trailing `.0` (e.g. `3` not `3.0`).
    let no_type = fs.type_char.is_none();
    let component_type = match fs.type_char {
        Some('n') => Some('g'),
        None if fs.precision.is_some() => Some('g'),
        other => other,
    };

    // Per-component sub-spec: keep sign / alt / precision / grouping / type,
    // but strip width / zero-pad / fill / align (those apply to the whole
    // assembled string, not the individual components).
    let make_component = |part: f64, sign: Option<char>| -> Result<String> {
        let sub = FormatSpec {
            fill: ' ',
            align: None,
            fill_explicit: false,
            sign,
            alt: fs.alt,
            zero_pad: false,
            width: 0,
            grouping: fs.grouping,
            precision: fs.precision,
            type_char: component_type,
        };
        let mut s = format_float_value(&Value::float(part), &sub, component_type, "complex")?;
        // Repr-style (no type, no precision): drop the trailing `.0` that the
        // float formatter emits for integer-valued floats.  With the alternate
        // form the decimal point is retained (`3.` not `3.0`); CPython keeps
        // the point but not the zero.
        if no_type
            && component_type.is_none()
            && let Some(stripped) = s.strip_suffix(".0")
        {
            s = if fs.alt {
                format!("{stripped}.")
            } else {
                stripped.to_string()
            };
        }
        Ok(s)
    };

    // The imaginary part always carries an explicit sign separator.  The float
    // formatter drops the forced `+` for nan / inf (its special-value branch
    // ignores the sign flag), so re-assert it here to match CPython's
    // `inf+nanj` form.
    let imag_component = |part: f64| -> Result<String> {
        let s = make_component(part, Some('+'))?;
        if s.starts_with('+') || s.starts_with('-') {
            Ok(s)
        } else {
            Ok(format!("+{s}"))
        }
    };

    let body = if no_type {
        // No presentation type: repr-style components, parenthesised unless the
        // real part is positive zero (then only the imaginary part is shown).
        if re == 0.0 && (1.0_f64).copysign(re) > 0.0 {
            // Pure-imaginary form: the imaginary part follows the spec's sign
            // flag (no forced `+` separator, since no real part precedes it).
            let im_str = make_component(im, fs.sign)?;
            format!("{im_str}j")
        } else {
            let re_str = make_component(re, fs.sign)?;
            let im_str = imag_component(im)?;
            format!("({re_str}{im_str}j)")
        }
    } else {
        // Presentation type given: format both parts, no parentheses.
        let re_str = make_component(re, fs.sign)?;
        let im_str = imag_component(im)?;
        format!("{re_str}{im_str}j")
    };

    // CPython right-aligns complex on width (numeric default).
    Ok(pad_value(&body, fs, '>', fs.fill))
}
