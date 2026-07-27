fn sign_prefix_for(negative: bool, sign: Option<char>) -> &'static str {
    if negative {
        return "-";
    }
    match sign {
        Some('+') => "+",
        Some(' ') => " ",
        _ => "",
    }
}

/// Insert grouping characters into a digit string (right-to-left).
///
/// `digits` is always ASCII (decimal / hex / oct / bin digits) and `sep` is the
/// ASCII `,` or `_` separator, so the work is done over bytes in a single
/// pre-sized allocation rather than collecting a `Vec<char>` and reversing it
/// twice.
fn group_digits(digits: &str, sep: char, group_size: usize) -> String {
    let digits = digits.as_bytes();
    let len = digits.len();
    let sep_count = len.saturating_sub(1) / group_size;
    let mut out = vec![0u8; len + sep_count];
    let sep_byte = sep as u8;
    // Walk source and destination from the right, inserting a separator every
    // `group_size` source digits.
    let mut dst = out.len();
    for (i, &c) in digits.iter().rev().enumerate() {
        if i > 0 && i % group_size == 0 {
            dst -= 1;
            out[dst] = sep_byte;
        }
        dst -= 1;
        out[dst] = c;
    }
    // `out` is all ASCII (digits + separator).
    String::from_utf8(out).expect("ascii grouped digits")
}

/// Apply decimal grouping to the integer part of a float body (e.g. "1234.50"
/// → "1,234.50").  Leaves any exponent / suffix portion intact.
fn group_float_int_part(body: &str, sep: char) -> String {
    // Find the integer portion: up to the first '.' or 'e' / 'E' or '%'.
    let mut end = body.len();
    for (i, c) in body.char_indices() {
        if matches!(c, '.' | 'e' | 'E' | '%') {
            end = i;
            break;
        }
    }
    let (int_part, rest) = body.split_at(end);
    let grouped = group_digits(int_part, sep, 3);
    format!("{grouped}{rest}")
}

/// Assemble the final string with sign / alt-prefix / body and apply width
/// + alignment + zero-pad rules.
///
/// `group_size` controls how zero-pad interleaves with the grouping
/// separator: `3` for decimal / float grouping, `4` for `_` grouping with
/// non-decimal integer bases (`b`/`o`/`x`/`X`), matching CPython.
fn assemble_numeric(
    sign_prefix: &str,
    alt_prefix: &str,
    body: String,
    fs: &FormatSpec,
    default_align: char,
    group_size: usize,
) -> String {
    let raw_len = sign_prefix.chars().count() + alt_prefix.chars().count() + body.chars().count();

    // Determine effective alignment.  If zero-pad is set and no explicit
    // align was given, alignment becomes '=' (pad between sign/prefix and
    // digits).
    let effective_align = if let Some(a) = fs.align {
        a
    } else if fs.zero_pad {
        '='
    } else {
        default_align
    };
    // Zero-pad promotes fill to '0' unless the user explicitly supplied a
    // fill character via the two-character `[fill]align` form.
    let effective_fill = if fs.zero_pad && !fs.fill_explicit {
        '0'
    } else {
        fs.fill
    };

    if fs.width == 0 || raw_len >= fs.width {
        return format!("{sign_prefix}{alt_prefix}{body}");
    }
    let pad = fs.width - raw_len;
    let fill_str: String = std::iter::repeat_n(effective_fill, pad).collect();

    match effective_align {
        '=' => {
            // sign + prefix + fill + body
            let body_grouped = if let (true, Some(grouping)) = (effective_fill == '0', fs.grouping)
            {
                // CPython interleaves the grouping separator with the zero
                // pad characters so the resulting body still groups in
                // threes-or-fours.  Apply by left-padding the body with
                // zeros first, then re-grouping the integer portion.
                regroup_with_zero_pad(&body, pad, grouping, group_size, alt_prefix)
            } else {
                let mut s = String::with_capacity(pad + body.len());
                s.push_str(&fill_str);
                s.push_str(&body);
                s
            };
            format!("{sign_prefix}{alt_prefix}{body_grouped}")
        }
        '>' => format!("{fill_str}{sign_prefix}{alt_prefix}{body}"),
        '<' => format!("{sign_prefix}{alt_prefix}{body}{fill_str}"),
        '^' => {
            let left = pad / 2;
            let right = pad - left;
            let left_fill: String = std::iter::repeat_n(effective_fill, left).collect();
            let right_fill: String = std::iter::repeat_n(effective_fill, right).collect();
            format!("{left_fill}{sign_prefix}{alt_prefix}{body}{right_fill}")
        }
        _ => format!("{sign_prefix}{alt_prefix}{body}"),
    }
}

/// When zero-padding combines with thousands grouping (e.g. `{-12345:08,}` ->
/// `-012,345`), CPython expands the integer portion with zeros then regroups
/// so the leading zeros are themselves separated by the group char.  The
/// final grouped string must be at least `current_int_len + pad` characters
/// long.
///
/// `group_size` is `3` for decimal grouping (`,` or `_` with `d`/`n`/no type
/// or with floats), and `4` for `_` grouping with non-decimal integer bases
/// (`b`/`o`/`x`/`X`), matching CPython's rules.
fn regroup_with_zero_pad(
    body: &str,
    pad: usize,
    sep: char,
    group_size: usize,
    _alt_prefix: &str,
) -> String {
    // Split body into integer / fractional / suffix parts.  For non-decimal
    // integer bases (group_size == 4) the body is hex/oct/bin digits only —
    // including `e`, which is a legitimate hex digit — so skip the split.
    // For decimal / float (group_size == 3) the body may contain `.`, `e`,
    // `E`, or `%` which mark the end of the integer portion.
    let (int_part, rest) = if group_size == 3 {
        match body.find(['.', 'e', 'E', '%']) {
            Some(i) => (&body[..i], &body[i..]),
            None => (body, ""),
        }
    } else {
        (body, "")
    };

    // Count the bare digits (separators stripped) in the integer part.
    let bare_len = int_part.chars().filter(|c| *c != sep).count();
    let target_int_len = int_part.chars().count() + pad;

    // Determine the number of zero-padded digits `n` (>= bare_len) needed so the
    // grouped length — `n` digits plus `(n - 1) / group_size` separators —
    // reaches `target_int_len`.  This is the closed form of the old
    // prepend-a-zero-then-regroup loop, computed without re-grouping each step.
    // `grouped_len(0)` is 0; `grouped_len(n>=1)` is `n + (n - 1) / group_size`.
    let grouped_len = |n: usize| if n == 0 { 0 } else { n + (n - 1) / group_size };
    let mut n = bare_len;
    while grouped_len(n) < target_int_len {
        n += 1;
    }
    let zeros = n - bare_len;

    // Emit directly into a single pre-sized buffer.  Build the grouped digits
    // right-to-left (original digits then the synthetic leading zeros) into a
    // byte vec — both digits and the separator are ASCII — then reverse once.
    let sep_count = n.saturating_sub(1) / group_size;
    let mut out_rev: Vec<u8> = Vec::with_capacity(n + sep_count + rest.len());
    let mut emitted = 0usize;
    let sep_byte = sep as u8;
    let push_digit = |out_rev: &mut Vec<u8>, emitted: &mut usize, d: u8| {
        if *emitted > 0 && (*emitted).is_multiple_of(group_size) {
            out_rev.push(sep_byte);
        }
        out_rev.push(d);
        *emitted += 1;
    };
    for c in int_part.bytes().rev() {
        if c == sep_byte {
            continue;
        }
        push_digit(&mut out_rev, &mut emitted, c);
    }
    for _ in 0..zeros {
        push_digit(&mut out_rev, &mut emitted, b'0');
    }
    out_rev.reverse();
    out_rev.extend_from_slice(rest.as_bytes());

    // out_rev holds only ASCII (grouped digits + separators) plus `rest`, which
    // for the float/decimal path is the original ASCII fractional/exponent tail.
    String::from_utf8(out_rev).expect("ascii grouped digits with ascii tail")
}

/// When the alternate form '#' is given to f/e/E/%, force a decimal point in
/// the body even if precision was 0.
fn ensure_alt_float(s: String, alt: bool, precision: Option<usize>) -> String {
    if !alt {
        return s;
    }
    if precision == Some(0) && !s.contains('.') {
        // Insert '.' before exponent if present, else append.
        if let Some(e_pos) = s.find(['e', 'E']) {
            let (a, b) = s.split_at(e_pos);
            format!("{a}.{b}")
        } else {
            format!("{s}.")
        }
    } else {
        s
    }
}

/// Pad a string-typed value per the format spec.
fn pad_value(raw: &str, fs: &FormatSpec, default_align: char, fill: char) -> String {
    let raw_len = raw.chars().count();
    if fs.width == 0 || raw_len >= fs.width {
        return raw.to_string();
    }
    let pad = fs.width - raw_len;
    let align = fs.align.unwrap_or(default_align);
    let fill_str: String = std::iter::repeat_n(fill, pad).collect();
    match align {
        '>' => format!("{fill_str}{raw}"),
        '<' => format!("{raw}{fill_str}"),
        '^' => {
            let left = pad / 2;
            let right = pad - left;
            let left_fill: String = std::iter::repeat_n(fill, left).collect();
            let right_fill: String = std::iter::repeat_n(fill, right).collect();
            format!("{left_fill}{raw}{right_fill}")
        }
        _ => format!("{raw}{fill_str}"),
    }
}

/// Normalise Rust's e-notation digits to Python's: always at least two
/// exponent digits and an explicit sign.
fn normalise_exp_digits(s: String) -> String {
    let e_pos = match s.find(['e', 'E']) {
        Some(p) => p,
        None => return s,
    };
    let (mantissa, exp_part) = s.split_at(e_pos);
    let e_char = &exp_part[..1];
    let exp_digits = &exp_part[1..];
    let (exp_sign, exp_num) = if exp_digits.starts_with('+') || exp_digits.starts_with('-') {
        (&exp_digits[..1], &exp_digits[1..])
    } else {
        ("+", exp_digits)
    };
    let exp_num_padded = if exp_num.len() < 2 {
        format!("0{exp_num}")
    } else {
        exp_num.to_string()
    };
    format!("{mantissa}{e_char}{exp_sign}{exp_num_padded}")
}

/// Coerce a `Value` to `f64` for format-spec (`format(x, ".2f")` / f-string)
/// numeric formatting.  Thin wrapper around [`try_value_to_float`] that
/// reports the format-path CPython-parity error message.
///
/// Raises `OverflowError` (not `TypeError`) when a `BigInt` argument overflows
/// f64 range, matching CPython's behaviour for `format(2**10000, ".2f")`.
fn fmt_value_to_float(value: &Value) -> Result<f64> {
    if let ValueKind::BigInt(b) = value.kind() {
        use crate::value::PyToPrimitive;
        let f = b.to_f64().unwrap_or(f64::INFINITY);
        return if f.is_finite() {
            Ok(f)
        } else {
            Err(pyrust_core::overflow_err!(
                "int too large to convert to float"
            ))
        };
    }
    try_value_to_float(value).ok_or_else(|| {
        pyrust_core::type_err!("must be real number, not {}", value_type_name_str(value))
    })
}

fn apply_sign_str(s: String, f: f64, sign: Option<char>) -> String {
    match sign {
        Some('+') if f >= 0.0 && !s.starts_with('-') => format!("+{s}"),
        Some(' ') if f >= 0.0 && !s.starts_with('-') => format!(" {s}"),
        _ => s,
    }
}

fn normalise_exp_str(s: String, f: f64, sign: Option<char>) -> String {
    // Rust: 1.23e5; Python: 1.23e+05 — adjust exponent format.
    let s = if let Some(e_pos) = s.find('e').or_else(|| s.find('E')) {
        let (mantissa, exp_part) = s.split_at(e_pos);
        let e_char = &exp_part[..1];
        let exp_digits = &exp_part[1..];
        // exp_digits starts with optional sign then digits
        let (exp_sign, exp_num) = if exp_digits.starts_with('+') || exp_digits.starts_with('-') {
            (&exp_digits[..1], &exp_digits[1..])
        } else {
            ("+", exp_digits)
        };
        // Python always uses at least 2 digits for the exponent (e.g. e+05 not e+5).
        let exp_num_padded = if exp_num.len() < 2 {
            format!("0{exp_num}")
        } else {
            exp_num.to_string()
        };
        format!("{mantissa}{e_char}{exp_sign}{exp_num_padded}")
    } else {
        s
    };
    apply_sign_str(s, f, sign)
}

/// Format a float with the no-type-char rule when a precision is given.
///
/// CPython's `{:.N}` (no explicit type) on a float differs from `{:.Ng}` in:
///
/// 1. Exponential threshold: switches to `e` notation when `exp >= max(N-1, 0)`
///    (one step earlier than `g`'s `exp >= N`).
/// 2. Fixed notation result must have at least one decimal digit (trailing `.0`
///    appended when the sig-fig trim would leave a bare integer like `10`).
///
/// `prec=0` is normalised to `prec=1` for the sig-fig computation but keeps
/// the `prec=0` threshold (i.e. `exp >= 0` triggers exponential).
///
/// Importantly, both threshold checks (`exp < -4` and `exp >= threshold`) use
/// the exponent of the *rounded* value, not the original.  For example,
/// `9.99` rounded to 1 sig fig is `10` (exp=1), so with prec=2 (threshold=1)
/// it triggers exponential notation even though the original exp was 0.
fn format_no_type_with_prec(f: f64, prec: usize) -> String {
    let sig_prec = if prec == 0 { 1 } else { prec };
    // Threshold: exp >= max(prec - 1, 0).  For prec=0 and prec=1 this is 0;
    // for prec >= 2 it is prec - 1.
    let threshold = if prec <= 1 { 0_i32 } else { (prec as i32) - 1 };

    if f == 0.0 {
        // Zero with prec <= 1 uses exponential: '0e+00'.
        // Zero with prec >= 2 uses fixed: '0.0'.
        return if threshold == 0 {
            "0e+00".to_string()
        } else {
            "0.0".to_string()
        };
    }

    // Format in exponential notation first to get the rounded exponent.
    // This correctly handles cases where rounding changes the order of
    // magnitude (e.g. 9.99 rounded to 1 sig fig => 10, exp becomes 1).
    let sig_digits = sig_prec.saturating_sub(1);
    let exp_str = format!("{:.sig_digits$e}", f);
    // Parse the exponent from Rust's exponential string (e.g. "1e1" -> 1).
    let rounded_exp = if let Some(e_pos) = exp_str.find('e') {
        exp_str[e_pos + 1..].parse::<i32>().unwrap_or(0)
    } else {
        0
    };

    if rounded_exp < -(4_i32) || rounded_exp >= threshold {
        // Exponential notation: reuse the already-computed exp_str.
        trim_g_trailing_zeros(normalise_exp_str(exp_str, f, None))
    } else {
        // Fixed notation.  Compute decimal places for sig_prec sig figs
        // using the rounded exponent so the digit count is correct.
        let decimal_digits = if rounded_exp >= 0 {
            sig_prec.saturating_sub(rounded_exp as usize + 1)
        } else {
            sig_prec + (-rounded_exp - 1) as usize
        };
        let s = format!("{:.decimal_digits$}", f);
        let s = trim_g_trailing_zeros(s);
        // Ensure at least one digit after the decimal point.
        if s.contains('.') || s.contains('e') {
            s
        } else {
            format!("{s}.0")
        }
    }
}

/// CPython's `%g` / `format(_, 'g')` algorithm, shared by `str.format`/`format()`
/// and `%`-printf (`expr.rs::format_general_float` delegates here).
///
/// Faithful port of CPython's general-format rounding rule:
///   1. Round the value to `prec` significant digits FIRST (via Rust's `{:.Ne}`,
///      which itself rounds).
///   2. Read the decimal exponent X of the *rounded* value — rounding can bump
///      the magnitude across a power of ten (e.g. `999999.5` -> `1e+06`), which
///      must change the fixed-vs-exponent decision (#2000).
///   3. Use fixed notation iff `-4 <= X < prec`, else exponential.
///   4. `alt` (the `#` form) keeps trailing zeros out to `prec` significant
///      figures and always keeps the decimal point; otherwise strip them (#1950).
///
/// `f` must be finite and non-NaN; callers handle inf/nan/sign separately.
fn format_g(f: f64, prec: usize, upper: bool, alt: bool) -> String {
    if f == 0.0 {
        // Zero's exponent is taken as 0, so it is always fixed notation.
        if alt {
            let mut out = String::from("0.");
            if prec > 1 {
                for _ in 0..prec - 1 {
                    out.push('0');
                }
            }
            return out;
        }
        return "0".to_string();
    }

    // Round to `prec` significant digits via exponential formatting, then read
    // the rounded exponent. This correctly handles rounding that crosses a
    // power of ten (#2000).
    let sig_digits = prec.saturating_sub(1);
    let exp_str = format!("{:.sig_digits$e}", f);
    let rounded_exp = if let Some(pos) = exp_str.find('e') {
        exp_str[pos + 1..].parse::<i32>().unwrap_or(0)
    } else {
        0
    };

    if rounded_exp < -(4_i32) || rounded_exp >= prec as i32 {
        // Exponential notation. Reuse the already-rounded mantissa string.
        let s = if upper {
            format!("{:.sig_digits$E}", f)
        } else {
            exp_str
        };
        let s = normalise_exp_str(s, f, None);
        if alt {
            ensure_exp_alt_zeros(s, prec)
        } else {
            trim_g_trailing_zeros(s)
        }
    } else {
        // Fixed notation: `prec` significant figures means
        //   decimal_digits = prec - 1 - exp (clamped at 0).
        let decimal_digits = if rounded_exp >= 0 {
            prec.saturating_sub(rounded_exp as usize + 1)
        } else {
            prec + (-rounded_exp - 1) as usize
        };
        let s = format!("{:.decimal_digits$}", f);
        if alt {
            // The rounded fixed string already carries exactly `prec`
            // significant digits; just guarantee a trailing decimal point.
            if s.contains('.') { s } else { format!("{s}.") }
        } else {
            trim_g_trailing_zeros(s)
        }
    }
}

/// Pad an already-normalised exponential string (`mantissa` + `e[+-]NN`) so its
/// mantissa carries `prec` significant digits, for the `#g`/`#G` alternate
/// form.  CPython keeps trailing zeros and the decimal point in this mode.
fn ensure_exp_alt_zeros(s: String, prec: usize) -> String {
    let e_pos = match s.find(['e', 'E']) {
        Some(p) => p,
        None => return s,
    };
    let (mantissa, exp_part) = s.split_at(e_pos);
    let sig: usize = mantissa.chars().filter(|c| c.is_ascii_digit()).count();
    let mut m = mantissa.to_string();
    if !m.contains('.') {
        m.push('.');
    }
    for _ in sig..prec {
        m.push('0');
    }
    format!("{m}{exp_part}")
}

fn trim_g_trailing_zeros(s: String) -> String {
    // Trim trailing zeros after decimal point (but keep 'e' part intact).
    let (mantissa, exp_part) = if let Some(e_pos) = s.find('e').or_else(|| s.find('E')) {
        (&s[..e_pos], &s[e_pos..])
    } else {
        (s.as_str(), "")
    };
    if mantissa.contains('.') {
        let trimmed = mantissa.trim_end_matches('0').trim_end_matches('.');
        format!("{trimmed}{exp_part}")
    } else {
        s
    }
}
