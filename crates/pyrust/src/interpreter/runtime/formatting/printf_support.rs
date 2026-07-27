// Pure helpers shared by string and bytes `%` formatting.
///
/// Append `data` to `out`, applying printf width padding with spaces.
///
/// `%b` / `%s` / `%a` / `%c` never zero-fill (CPython only zero-fills numeric
/// codes), so this only handles space padding plus left/right alignment.
fn bytes_printf_apply_width(
    out: &mut Vec<u8>,
    data: &[u8],
    width: Option<usize>,
    left_align: bool,
) {
    let w = match width {
        None | Some(0) => {
            out.extend_from_slice(data);
            return;
        }
        Some(w) => w,
    };
    if data.len() >= w {
        out.extend_from_slice(data);
        return;
    }
    let pad = w - data.len();
    if left_align {
        out.extend_from_slice(data);
        out.extend(std::iter::repeat_n(b' ', pad));
    } else {
        out.extend(std::iter::repeat_n(b' ', pad));
        out.extend_from_slice(data);
    }
}

/// Extract a single byte from a bytes-like for `%c`, or raise the CPython
/// `TypeError` for an empty or multi-byte argument.
fn single_byte_or_err(data: &[u8]) -> Result<u8> {
    if data.len() == 1 {
        Ok(data[0])
    } else {
        Err(pyrust_core::type_err!(
            "%c requires an integer in range(256) or a single byte"
        ))
    }
}

/// Take the next positional argument for printf-style formatting.
fn str_printf_take_positional(positional: &Option<&[Value]>, idx: &mut usize) -> Result<Value> {
    match positional {
        None => Err(pyrust_core::type_err!(
            "not enough arguments for format string"
        )),
        Some(items) => {
            if *idx >= items.len() {
                Err(pyrust_core::type_err!(
                    "not enough arguments for format string"
                ))
            } else {
                let v = items[*idx].clone();
                *idx += 1;
                Ok(v)
            }
        }
    }
}

/// Result of coercing a printf argument to an integer value.
///
/// `Small` covers values that fit in `i64` (the common case: `int`, `bool`,
/// truncated `float`).  `Big` is used only for `BigInt` values that are
/// outside the `i64` range — the caller formats them with BigInt-native
/// methods (`to_str_radix`, etc.) instead of Rust integer formatting.
enum PrintfInt {
    Small(i64),
    Big(PyBigInt),
}

/// Convert a `Value` to a `PrintfInt` for integer printf format codes.
///
/// Unlike the old `i64`-returning version, the `BigInt` arm no longer raises
/// `OverflowError`; it returns `PrintfInt::Big` so that the caller can format
/// arbitrarily large integers using BigInt-native methods.
///
/// For `%d`/`%i`/`%u`, float arguments are truncated toward zero following
/// CPython's `int(float)` semantics: NaN raises `ValueError`, infinity raises
/// `OverflowError`, and finite floats larger than `i64::MAX` are promoted to
/// `PrintfInt::Big` rather than being silently clamped.
fn str_printf_to_int(v: &Value, conv: char, bytes_mode: bool) -> Result<PrintfInt> {
    // CPython's bytes %-formatter normalises the %i alias to %d in its error
    // messages (the str formatter keeps %i); mirror that so the wording is
    // byte-identical to CPython 3.12 for both receivers.
    let disp = if bytes_mode && conv == 'i' { 'd' } else { conv };
    match v.kind() {
        ValueKind::Int(n) => Ok(PrintfInt::Small(n)),
        ValueKind::Bool(b) => Ok(PrintfInt::Small(b as i64)),
        ValueKind::Float(_) if matches!(conv, 'o' | 'x' | 'X') => {
            // CPython 3.12: %o/%x/%X reject float with "an integer is required".
            // %d/%i/%u accept float (truncating toward zero) for historical reasons.
            Err(pyrust_core::type_err!(
                "%{disp} format: an integer is required, not float"
            ))
        }
        ValueKind::Float(f) => {
            // CPython converts via PyLong_FromDouble: NaN → ValueError,
            // infinity → OverflowError, finite → truncate toward zero.
            // Rust's `f as i64` silently saturates at i64::MAX/MIN for
            // out-of-range finite floats, losing significant digits.
            let int_val = float_to_bigint(f)?;
            match int_val.kind() {
                ValueKind::Int(n) => Ok(PrintfInt::Small(n)),
                ValueKind::BigInt(b) => Ok(PrintfInt::Big(b.clone())),
                _ => unreachable!("float_to_bigint returns Int or BigInt"),
            }
        }
        ValueKind::BigInt(b) => Ok(PrintfInt::Big(b.clone())),
        _ => {
            // CPython uses "a real number is required" for %d/%i/%u,
            // and "an integer is required" for %o/%x/%X.
            let msg = if matches!(conv, 'o' | 'x' | 'X') {
                format!(
                    "%{disp} format: an integer is required, not {}",
                    pyrust_core::builtin_type_name(v)
                )
            } else {
                format!(
                    "%{disp} format: a real number is required, not {}",
                    pyrust_core::builtin_type_name(v)
                )
            };
            Err(pyrust_core::type_err!(msg))
        }
    }
}

/// Format a `BigInt` value for `%o`/`%x`/`%X` printf codes.
///
/// `to_str_radix` produces sign-magnitude notation (e.g., `-ff` for `-255`),
/// which matches CPython's behaviour.  This helper inserts the optional base
/// prefix (`0o`/`0x`/`0X`) and sign prefix (`+` / ` `) in the positions that
/// `apply_printf_width` expects for correct zero-fill later.
fn format_printf_bigint_radix(
    b: &PyBigInt,
    radix: u32,
    base_prefix: &str,
    upper: bool,
    flag_hash: bool,
    flag_plus: bool,
    flag_space: bool,
) -> String {
    // num_bigint::BigInt::to_str_radix uses sign-magnitude: negative values
    // get a leading '-'; the remaining digits are the absolute magnitude.
    let raw = b.to_str_radix(radix);
    let is_neg = raw.starts_with('-');
    let digits: std::borrow::Cow<str> = if upper {
        let d = if is_neg { &raw[1..] } else { &raw[..] };
        std::borrow::Cow::Owned(d.to_uppercase())
    } else if is_neg {
        std::borrow::Cow::Borrowed(&raw[1..])
    } else {
        std::borrow::Cow::Borrowed(&raw[..])
    };
    if is_neg {
        if flag_hash {
            format!("-{}{}", base_prefix, digits)
        } else {
            format!("-{}", digits)
        }
    } else if flag_hash {
        if flag_plus {
            format!("+{}{}", base_prefix, digits)
        } else if flag_space {
            format!(" {}{}", base_prefix, digits)
        } else {
            format!("{}{}", base_prefix, digits)
        }
    } else if flag_plus {
        format!("+{}", digits)
    } else if flag_space {
        format!(" {}", digits)
    } else {
        digits.into_owned()
    }
}

/// Convert a `Value` to `f64` for float printf format codes.
fn str_printf_to_float(v: &Value, _conv: char, bytes_mode: bool) -> Result<f64> {
    match v.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(n) => Ok(n as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        ValueKind::BigInt(b) => bigint_to_float_or_overflow(b),
        // CPython's bytes %-formatter reports "float argument required, not X"
        // for the float codes (%e/%E/%f/%F/%g/%G), whereas the str formatter
        // reports "must be real number, not X".
        _ if bytes_mode => Err(pyrust_core::type_err!(
            "float argument required, not {}",
            pyrust_core::builtin_type_name(v)
        )),
        _ => Err(pyrust_core::type_err!(
            "must be real number, not {}",
            pyrust_core::builtin_type_name(v)
        )),
    }
}

/// Truncate a string to `precision` Unicode chars (for `%s` and `%r`).
fn apply_str_precision(s: String, precision: Option<usize>) -> String {
    match precision {
        None => s,
        Some(max_chars) => {
            if s.chars().count() <= max_chars {
                s
            } else {
                s.chars().take(max_chars).collect()
            }
        }
    }
}

/// Apply width padding to a formatted value string.
fn apply_printf_width(
    s: String,
    width: Option<usize>,
    left_align: bool,
    zero_fill: bool,
    conv: char,
) -> String {
    let w = match width {
        None | Some(0) => return s,
        Some(w) => w,
    };
    let char_len = s.chars().count();
    if char_len >= w {
        return s;
    }
    let pad = w - char_len;
    // Zero-fill only for numeric codes, not %s/%r/%c, and not with left-align.
    if zero_fill
        && !left_align
        && matches!(
            conv,
            'd' | 'i' | 'u' | 'o' | 'x' | 'X' | 'f' | 'e' | 'E' | 'g' | 'G'
        )
    {
        // Determine the non-digit prefix: optional sign (+/-/space), then
        // optional base prefix (0x, 0X, 0o).  Zeros are inserted after the
        // full prefix so that "%#010x" % 255 → "0x000000ff" not "0000000xff".
        let prefix_len = {
            let mut cs = s.chars();
            let mut n = 0usize;
            // sign
            if let Some('+' | '-' | ' ') = cs.next() {
                n += 1;
                // base prefix after sign: 0x, 0X, 0o
                let mut peek = s[n..].chars();
                if peek.next() == Some('0') && matches!(peek.next(), Some('x' | 'X' | 'o')) {
                    n += 2;
                }
            } else if s.starts_with("0x") || s.starts_with("0X") || s.starts_with("0o") {
                n = 2;
            }
            n
        };
        let mut out = String::with_capacity(w);
        out.push_str(&s[..prefix_len]);
        for _ in 0..pad {
            out.push('0');
        }
        out.push_str(&s[prefix_len..]);
        return out;
    }
    if left_align {
        let mut out = s;
        for _ in 0..pad {
            out.push(' ');
        }
        out
    } else {
        let mut out = String::with_capacity(w);
        for _ in 0..pad {
            out.push(' ');
        }
        out.push_str(&s);
        out
    }
}

/// Format a float in scientific notation matching CPython's `%e`/`%E`.
///
/// CPython always uses a sign and at least two exponent digits: e+03, e-03.
/// Rust's default format may omit the sign for positive exponents; this
/// function normalises the output to match CPython.
fn format_scientific(f: f64, prec: usize, upper: bool) -> String {
    if f.is_nan() {
        return if upper {
            "NAN".to_string()
        } else {
            "nan".to_string()
        };
    }
    if f.is_infinite() {
        return if f > 0.0 {
            if upper {
                "INF".to_string()
            } else {
                "inf".to_string()
            }
        } else if upper {
            "-INF".to_string()
        } else {
            "-inf".to_string()
        };
    }
    let raw = format!("{:.prec$e}", f, prec = prec);
    let e_char = if upper { 'E' } else { 'e' };
    if let Some(pos) = raw.find('e') {
        let mantissa = &raw[..pos];
        let exp_str = &raw[pos + 1..];
        let digits = if exp_str.starts_with(['-', '+']) {
            &exp_str[1..]
        } else {
            exp_str
        };
        let sign: i32 = if exp_str.starts_with('-') { -1 } else { 1 };
        let exp_n: i32 = digits.parse::<i32>().unwrap_or(0) * sign;
        // {:+03} produces "+03", "-03" — sign always included, at least 2 digits.
        format!("{}{}{:+03}", mantissa, e_char, exp_n)
    } else {
        raw
    }
}

/// `%g` / `%G` printf conversion. Delegates the digit/exponent logic to the
/// shared `format_g` so the `%`-printf path and `str.format` path agree
/// on rounding-then-exponent (#2000) and `#`-alternate trailing zeros (#1950).
/// `alt` is the printf `#` flag.
fn format_general_float(f: f64, prec: usize, upper: bool, alt: bool) -> String {
    if f.is_nan() {
        return if upper {
            "NAN".to_string()
        } else {
            "nan".to_string()
        };
    }
    if f.is_infinite() {
        return if f > 0.0 {
            if upper {
                "INF".to_string()
            } else {
                "inf".to_string()
            }
        } else if upper {
            "-INF".to_string()
        } else {
            "-inf".to_string()
        };
    }
    let body = format_g(f.abs(), prec, upper, alt);
    if f.is_sign_negative() {
        format!("-{body}")
    } else {
        body
    }
}
