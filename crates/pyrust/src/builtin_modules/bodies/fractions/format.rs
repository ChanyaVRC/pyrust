//! `Fraction.__format__`: CPython 3.12's float-style format specifications
//! (`e`, `E`, `f`, `F`, `g`, `G`, `%`) rendered exactly from the rational
//! value.
//!
//! [`parse_spec`] is equivalent to `_FLOAT_FORMAT_SPECIFICATION_MATCHER`:
//!
//! ```text
//! (?:
//!     (?P<fill>.)?
//!     (?P<align>[<>=^])
//! )?
//! (?P<sign>[-+ ]?)
//! (?P<no_neg_zero>z)?
//! (?P<alt>\#)?
//! # A '0' that's *not* followed by another digit is parsed as a minimum width
//! # rather than a zeropad flag.
//! (?P<zeropad>0(?=[0-9]))?
//! (?P<minimumwidth>0|[1-9][0-9]*)?
//! (?P<thousands_sep>[,_])?
//! (?:\.(?P<precision>0|[1-9][0-9]*))?
//! (?P<presentation_type>[eEfFgG%])
//! ```
//!
//! applied with `fullmatch` under `re.DOTALL`.  The only backtracking choice
//! points are the zeropad flag and the width alternatives; they are tried in
//! the regex engine's order, so the captures are the ones CPython sees.

use num_bigint::BigInt;
use num_traits::Signed;

use super::arith::{int_str, round_to_exponent, round_to_figures};
use crate::error::{PyError, Result};

/// The groups of a matched format specification.
pub(super) struct Spec<'a> {
    pub(super) fill: Option<&'a str>,
    pub(super) align: Option<u8>,
    pub(super) sign: Option<u8>,
    pub(super) no_neg_zero: bool,
    pub(super) alternate: bool,
    pub(super) zeropad: bool,
    pub(super) minimumwidth: Option<&'a str>,
    pub(super) thousands_sep: Option<u8>,
    pub(super) precision: Option<&'a str>,
    pub(super) presentation_type: u8,
}

/// Byte length of the code point that starts with `lead` (UTF-8 and the
/// CESU-8 surrogate encoding share lead-byte lengths).
fn char_len(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

fn is_align(b: u8) -> bool {
    matches!(b, b'<' | b'>' | b'=' | b'^')
}

/// The groups after the optional zeropad flag, given the width choice.
struct Tail<'a> {
    minimumwidth: Option<&'a str>,
    thousands_sep: Option<u8>,
    precision: Option<&'a str>,
    presentation_type: u8,
}

/// `(?P<thousands_sep>[,_])?(?:\.(?P<precision>0|[1-9][0-9]*))?
/// (?P<presentation_type>[eEfFgG%])` followed by the end of the spec.
fn match_after_width<'a>(rest: &'a str, minimumwidth: Option<&'a str>) -> Option<Tail<'a>> {
    let bytes = rest.as_bytes();
    let mut i = 0;
    let thousands_sep = match bytes.first() {
        Some(&b @ (b',' | b'_')) => {
            i += 1;
            Some(b)
        }
        _ => None,
    };
    let mut precision = None;
    if bytes.get(i) == Some(&b'.') {
        let start = i + 1;
        let end = match bytes.get(start) {
            Some(b'0') => start + 1,
            Some(b'1'..=b'9') => {
                let mut end = start + 1;
                while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                    end += 1;
                }
                end
            }
            _ => return None,
        };
        precision = Some(&rest[start..end]);
        i = end;
    }
    match bytes.get(i) {
        Some(&t @ (b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'%')) if i + 1 == bytes.len() => {
            Some(Tail {
                minimumwidth,
                thousands_sep,
                precision,
                presentation_type: t,
            })
        }
        _ => None,
    }
}

/// `(?P<minimumwidth>0|[1-9][0-9]*)?` and everything after it, trying the
/// alternatives in the regex engine's order.
fn match_width(rest: &str) -> Option<Tail<'_>> {
    let bytes = rest.as_bytes();
    if bytes.first() == Some(&b'0')
        && let Some(tail) = match_after_width(&rest[1..], Some(&rest[..1]))
    {
        return Some(tail);
    }
    if bytes.first().is_some_and(|b| (b'1'..=b'9').contains(b)) {
        let mut end = 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        // A shorter width would leave a digit that nothing else can match.
        if let Some(tail) = match_after_width(&rest[end..], Some(&rest[..end])) {
            return Some(tail);
        }
    }
    match_after_width(rest, None)
}

/// Match `spec` against `_FLOAT_FORMAT_SPECIFICATION_MATCHER`.
pub(super) fn parse_spec(spec: &str) -> Option<Spec<'_>> {
    let bytes = spec.as_bytes();
    let first_len = bytes.first().map_or(0, |&b| char_len(b));
    let (fill, align, mut rest) = match (bytes.first(), bytes.get(first_len)) {
        (Some(_), Some(&a)) if is_align(a) => {
            (Some(&spec[..first_len]), Some(a), &spec[first_len + 1..])
        }
        (Some(&a), _) if is_align(a) => (None, Some(a), &spec[1..]),
        _ => (None, None, spec),
    };
    // Every remaining group is ASCII.
    if !rest.is_ascii() {
        return None;
    }
    let mut sign = None;
    if let Some(&s @ (b'-' | b'+' | b' ')) = rest.as_bytes().first() {
        sign = Some(s);
        rest = &rest[1..];
    }
    let no_neg_zero = rest.starts_with('z');
    if no_neg_zero {
        rest = &rest[1..];
    }
    let alternate = rest.starts_with('#');
    if alternate {
        rest = &rest[1..];
    }
    let rb = rest.as_bytes();
    let mut zeropad = false;
    let mut tail = None;
    if rb.first() == Some(&b'0') && rb.get(1).is_some_and(u8::is_ascii_digit) {
        tail = match_width(&rest[1..]);
        zeropad = tail.is_some();
    }
    let tail = match tail {
        Some(tail) => tail,
        None => match_width(rest)?,
    };
    Some(Spec {
        fill,
        align,
        sign,
        no_neg_zero,
        alternate,
        zeropad,
        minimumwidth: tail.minimumwidth,
        thousands_sep: tail.thousands_sep,
        precision: tail.precision,
        presentation_type: tail.presentation_type,
    })
}

fn memory_error() -> PyError {
    PyError::named("MemoryError", String::new())
}

/// A width/precision group as a `usize`, raising `MemoryError` for values no
/// output buffer could hold (CPython runs out of memory on them too).
fn spec_number(digits: &str) -> Result<usize> {
    digits
        .parse::<usize>()
        .ok()
        .filter(|&n| n <= isize::MAX as usize)
        .ok_or_else(memory_error)
}

fn repeat_fill(fill: &str, count: usize) -> Result<String> {
    let mut out = String::new();
    out.try_reserve(fill.len().saturating_mul(count))
        .map_err(|_| memory_error())?;
    for _ in 0..count {
        out.push_str(fill);
    }
    Ok(out)
}

/// The body of `Fraction.__format__` for a non-empty, matched `spec`
/// applied to `numerator / denominator`.
pub(super) fn format_fraction(
    spec: &Spec<'_>,
    numerator: &BigInt,
    denominator: &BigInt,
) -> Result<String> {
    let fill = spec.fill.unwrap_or(" ");
    let align = spec.align.unwrap_or(b'>');
    let pos_sign = match spec.sign {
        Some(b'+') => "+",
        Some(b' ') => " ",
        _ => "",
    };
    let minimumwidth = spec.minimumwidth.map_or(Ok(0), spec_number)?;
    let precision = spec.precision.map_or(Ok(6), spec_number)?;
    let ptype = spec.presentation_type;
    let trim_zeros = matches!(ptype, b'g' | b'G') && !spec.alternate;
    let trim_point = !spec.alternate;
    let exponent_indicator = if matches!(ptype, b'E' | b'F' | b'G') {
        'E'
    } else {
        'e'
    };

    // Round to get the digits we need, figure out where to place the point,
    // and decide whether to use scientific notation.  `point_pos` is the
    // number of digits that should follow the point.
    let precision_big = BigInt::from(precision);
    let (negative, significand, exponent, scientific, point_pos) =
        if matches!(ptype, b'f' | b'F' | b'%') {
            let mut exponent = -&precision_big;
            if ptype == b'%' {
                exponent -= 2;
            }
            let (negative, significand) =
                round_to_exponent(numerator, denominator, &exponent, spec.no_neg_zero)?;
            (negative, significand, exponent, false, precision)
        } else {
            let figures = if matches!(ptype, b'g' | b'G') {
                precision.max(1)
            } else {
                precision.checked_add(1).ok_or_else(memory_error)?
            };
            let figures_big = BigInt::from(figures);
            let (negative, significand, exponent) =
                round_to_figures(numerator, denominator, &figures_big)?;
            let scientific = matches!(ptype, b'e' | b'E')
                || exponent.is_positive()
                || &exponent + &figures_big <= BigInt::from(-4);
            let point_pos = if scientific {
                figures - 1
            } else {
                // Not scientific: -4 < exponent + figures and exponent <= 0.
                usize::try_from(-&exponent).map_err(|_| memory_error())?
            };
            (negative, significand, exponent, scientific, point_pos)
        };

    // Get the suffix - the part following the digits, if any.
    let suffix = if ptype == b'%' {
        "%".to_string()
    } else if scientific {
        let shown = &exponent + BigInt::from(point_pos);
        let sign = if shown.is_negative() { '-' } else { '+' };
        format!("{exponent_indicator}{sign}{:0>2}", int_str(&shown.abs())?)
    } else {
        String::new()
    };

    // String of output digits, padded sufficiently with zeros on the left so
    // that there is at least one digit before the decimal point.
    let raw_digits = int_str(&significand)?;
    let width = point_pos.checked_add(1).ok_or_else(memory_error)?;
    let digits = if raw_digits.len() < width {
        let mut padded = repeat_fill("0", width - raw_digits.len())?;
        padded.push_str(&raw_digits);
        padded
    } else {
        raw_digits
    };

    // Before padding, the output has the form f"{sign}{leading}{trailing}".
    let sign = if negative { "-" } else { pos_sign };
    let split = digits.len() - point_pos;
    let mut leading = digits[..split].to_string();
    let mut frac_part = &digits[split..];
    if trim_zeros {
        frac_part = frac_part.trim_end_matches('0');
    }
    let separator = if trim_point && frac_part.is_empty() {
        ""
    } else {
        "."
    };
    let trailing = format!("{separator}{frac_part}{suffix}");

    // Do zero padding if required.
    if spec.zeropad {
        let min_leading = minimumwidth as i128 - sign.len() as i128 - trailing.len() as i128;
        let target = if spec.thousands_sep.is_some() {
            (3 * min_leading).div_euclid(4) + 1
        } else {
            min_leading
        };
        if target > leading.len() as i128 {
            let mut padded = repeat_fill("0", target as usize - leading.len())?;
            padded.push_str(&leading);
            leading = padded;
        }
    }

    // Insert thousands separators if required.
    if let Some(sep) = spec.thousands_sep {
        let sep = sep as char;
        let first_pos = 1 + (leading.len() - 1) % 3;
        let mut grouped = String::with_capacity(leading.len() + leading.len() / 3);
        grouped.push_str(&leading[..first_pos]);
        let mut pos = first_pos;
        while pos < leading.len() {
            grouped.push(sep);
            grouped.push_str(&leading[pos..pos + 3]);
            pos += 3;
        }
        leading = grouped;
    }

    // We now have a sign and a body.  Pad with the fill character if
    // necessary.  `fill` is the only non-ASCII part, one code point long.
    let body_len = leading.len() + trailing.len();
    let pad = minimumwidth.saturating_sub(sign.len() + body_len);
    let padding = repeat_fill(fill, pad)?;
    let body = format!("{leading}{trailing}");
    Ok(match align {
        b'>' => format!("{padding}{sign}{body}"),
        b'<' => format!("{sign}{body}{padding}"),
        b'^' => {
            let half = pad / 2;
            let split = half * fill.len();
            format!("{}{sign}{body}{}", &padding[..split], &padding[split..])
        }
        _ => format!("{sign}{padding}{body}"),
    })
}
