/// Produce the digit-only "body" of an integer formatting (no sign / prefix).
fn int_body(magnitude: u64, type_char: char) -> String {
    match type_char {
        'd' => format!("{magnitude}"),
        'b' => format!("{magnitude:b}"),
        'o' => format!("{magnitude:o}"),
        'x' => format!("{magnitude:x}"),
        'X' => format!("{magnitude:X}"),
        _ => format!("{magnitude}"),
    }
}

/// Produce the digit-only body of a BigInt formatting (no sign / prefix).
fn bigint_body(magnitude: &PyBigInt, type_char: char) -> String {
    let radix = match type_char {
        'b' => 2,
        'o' => 8,
        'x' | 'X' => 16,
        _ => 10,
    };
    // magnitude is always non-negative here (callers strip the sign).
    let s = magnitude.to_str_radix(radix);
    if type_char == 'X' {
        s.to_uppercase()
    } else {
        s
    }
}

fn prefix_for(type_char: char, alt: bool) -> &'static str {
    if !alt {
        return "";
    }
    match type_char {
        'b' => "0b",
        'o' => "0o",
        'x' => "0x",
        'X' => "0X",
        _ => "",
    }
}

fn format_int_value(
    value: &Value,
    fs: &FormatSpec,
    type_char: Option<char>,
    type_name: &str,
) -> Result<String> {
    // BigInt is handled via a separate path that avoids the i128 narrowing.
    if let ValueKind::BigInt(b) = value.kind() {
        return format_bigint_value(b, fs, type_char);
    }

    let n: i128 = match value.kind() {
        ValueKind::Int(n) => n as i128,
        ValueKind::Bool(b) => {
            if b {
                1
            } else {
                0
            }
        }
        _ => {
            let code = type_char.unwrap_or('d');
            return Err(pyrust_core::value_err!(
                "Unknown format code '{}' for object of type '{type_name}'",
                format_code_repr(code)
            ));
        }
    };

    if fs.precision.is_some() {
        return Err(pyrust_core::value_err!(
            "Precision not allowed in integer format specifier"
        ));
    }

    let t = type_char.unwrap_or('d');

    // 'c': render as the unicode character.
    if t == 'c' {
        if fs.sign.is_some() || fs.alt || fs.grouping.is_some() {
            return Err(pyrust_core::value_err!(
                "Cannot specify ',' or '_', sign, or '#' with 'c'."
            ));
        }
        if !(0..=0x10FFFF).contains(&n) {
            return Err(pyrust_core::overflow_err!("%c arg not in range(0x110000)"));
        }
        let ch = char::from_u32(n as u32)
            .ok_or_else(|| pyrust_core::overflow_err!("%c arg not in range(0x110000)"))?;
        let raw = ch.to_string();
        return Ok(pad_value(&raw, fs, '<', fs.fill));
    }

    // 'n' already implies locale-aware grouping, so CPython rejects an
    // explicit ',' / '_' combined with it (reported against the original 'n'
    // type, not the effective 'd' it maps to).
    if let Some(g) = fs.grouping
        && t == 'n'
    {
        return Err(pyrust_core::value_err!("Cannot specify '{g}' with 'n'."));
    }

    // 'n' = same as 'd' for now (no locale-aware grouping).
    let effective_t = if t == 'n' { 'd' } else { t };

    // Validate grouping vs type.
    if let Some(g) = fs.grouping {
        let ok = match (g, effective_t) {
            (',', 'd') => true,
            (',', _) => false,
            ('_', 'd' | 'b' | 'o' | 'x' | 'X') => true,
            _ => false,
        };
        if !ok {
            return Err(pyrust_core::value_err!(
                "Cannot specify '{g}' with '{effective_t}'."
            ));
        }
    }

    let negative = n < 0;
    let magnitude: u64 = if negative {
        // i64::MIN edge case: -(i128) fits in u64 via wrap.
        (-n) as u64
    } else {
        n as u64
    };

    let sign_prefix = sign_prefix_for(negative, fs.sign);
    let alt_prefix = prefix_for(effective_t, fs.alt);
    let mut body = int_body(magnitude, effective_t);

    // Apply grouping to the digit body.  For non-decimal bases (b/o/x/X),
    // CPython groups every 4 digits with '_'.  For decimal, every 3 digits
    // with either ',' or '_'.
    let group_size = if effective_t == 'd' { 3 } else { 4 };
    if let Some(g) = fs.grouping {
        body = group_digits(&body, g, group_size);
    }

    // Apply zero-pad / width / alignment.  Pass `group_size` so that
    // zero-pad + grouping with non-decimal bases (e.g. `{:0_12x}`) re-groups
    // the zero-padded body every 4 digits rather than every 3.
    Ok(assemble_numeric(
        sign_prefix,
        alt_prefix,
        body,
        fs,
        // Numeric default alignment is right.
        '>',
        group_size,
    ))
}

/// Format a `BigInt` value according to an integer `FormatSpec`.
/// Mirrors `format_int_value` but uses `bigint_body` instead of the
/// `u64`-based `int_body` to avoid narrowing large values.
fn format_bigint_value(b: &PyBigInt, fs: &FormatSpec, type_char: Option<char>) -> Result<String> {
    if fs.precision.is_some() {
        return Err(pyrust_core::value_err!(
            "Precision not allowed in integer format specifier"
        ));
    }

    let t = type_char.unwrap_or('d');

    // 'c': a BigInt is almost certainly out of range, but check correctly.
    if t == 'c' {
        if fs.sign.is_some() || fs.alt || fs.grouping.is_some() {
            return Err(pyrust_core::value_err!(
                "Cannot specify ',' or '_', sign, or '#' with 'c'."
            ));
        }
        // A BigInt is by definition outside the C long range (> i64::MAX or
        // < i64::MIN), so it can never be a valid chr() argument.  CPython
        // raises "Python int too large to convert to C long" for such values
        // rather than the "%c arg not in range(0x110000)" it uses for
        // in-range negative integers.
        return Err(pyrust_core::overflow_err!(
            "Python int too large to convert to C long"
        ));
    }

    // 'n' already implies locale-aware grouping, so CPython rejects an
    // explicit ',' / '_' combined with it (reported against the original 'n'
    // type, not the effective 'd' it maps to).
    if let Some(g) = fs.grouping
        && t == 'n'
    {
        return Err(pyrust_core::value_err!("Cannot specify '{g}' with 'n'."));
    }

    // 'n' = same as 'd' for now (no locale-aware grouping).
    let effective_t = if t == 'n' { 'd' } else { t };

    // gh-95778: base-10 ('d'/'n') rendering of a BigInt is subject to the
    // int_max_str_digits limit; the power-of-two bases ('b'/'o'/'x'/'X') below
    // are exempt.
    if effective_t == 'd' && pyrust_core::bigint_str_digits_exceed_limit(b) {
        return Err(pyrust_core::int_max_str_digits_format_error());
    }

    // Validate grouping vs type.
    if let Some(g) = fs.grouping {
        let ok = match (g, effective_t) {
            (',', 'd') => true,
            (',', _) => false,
            ('_', 'd' | 'b' | 'o' | 'x' | 'X') => true,
            _ => false,
        };
        if !ok {
            return Err(pyrust_core::value_err!(
                "Cannot specify '{g}' with '{effective_t}'."
            ));
        }
    }

    use num_bigint::Sign;
    let negative = b.sign() == Sign::Minus;
    // magnitude: absolute value used for digit conversion.
    let magnitude = if negative { -b.clone() } else { b.clone() };

    let sign_prefix = sign_prefix_for(negative, fs.sign);
    let alt_prefix = prefix_for(effective_t, fs.alt);
    let mut body = bigint_body(&magnitude, effective_t);

    let group_size = if effective_t == 'd' { 3 } else { 4 };
    if let Some(g) = fs.grouping {
        body = group_digits(&body, g, group_size);
    }

    Ok(assemble_numeric(
        sign_prefix,
        alt_prefix,
        body,
        fs,
        '>',
        group_size,
    ))
}
