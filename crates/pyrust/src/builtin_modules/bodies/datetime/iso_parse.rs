//! ISO-8601 date, time, timezone, and datetime parsing for `datetime`.

use crate::error::{PyError, Result};

use super::calendar::{from_iso_cal, validate_ymd};

pub(super) type IsoDateTimeParts = (i64, i64, i64, i64, i64, i64, i64, Option<i64>);

pub(super) fn parse_ascii_digits(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut acc = 0i64;
    for &b in bytes {
        acc = acc * 10 + i64::from(b - b'0');
    }
    Some(acc)
}

fn invalid_iso(full: &str) -> PyError {
    PyError::named("ValueError", format!("Invalid isoformat string: '{full}'"))
}

pub(super) fn parse_iso_date_bytes(raw: &[u8], full: &str) -> Result<(i64, i64, i64)> {
    let (ymd, consumed) = parse_iso_date_prefix(raw, full)?;
    if consumed != raw.len() {
        return Err(invalid_iso(full));
    }
    Ok(ymd)
}

fn parse_iso_date_prefix(raw: &[u8], full: &str) -> Result<((i64, i64, i64), usize)> {
    let bad = || invalid_iso(full);
    if raw.len() >= 10 && raw[4] == b'-' && raw[5] == b'W' && raw[8] == b'-' {
        let y = parse_ascii_digits(&raw[0..4]).ok_or_else(bad)?;
        let w = parse_ascii_digits(&raw[6..8]).ok_or_else(bad)?;
        let d = parse_ascii_digits(&raw[9..10]).ok_or_else(bad)?;
        return Ok((from_iso_cal(y, w, d)?, 10));
    }
    if raw.len() >= 10 && raw[4] == b'-' && raw[7] == b'-' {
        let y = parse_ascii_digits(&raw[0..4]).ok_or_else(bad)?;
        let m = parse_ascii_digits(&raw[5..7]).ok_or_else(bad)?;
        let d = parse_ascii_digits(&raw[8..10]).ok_or_else(bad)?;
        validate_ymd(y, m, d)?;
        return Ok(((y, m, d), 10));
    }
    if raw.len() >= 8 && raw[4] == b'-' && raw[5] == b'W' {
        let y = parse_ascii_digits(&raw[0..4]).ok_or_else(bad)?;
        let w = parse_ascii_digits(&raw[6..8]).ok_or_else(bad)?;
        return Ok((from_iso_cal(y, w, 1)?, 8));
    }
    if raw.len() >= 8 && raw[4] == b'W' {
        let y = parse_ascii_digits(&raw[0..4]).ok_or_else(bad)?;
        let w = parse_ascii_digits(&raw[5..7]).ok_or_else(bad)?;
        let d = parse_ascii_digits(&raw[7..8]).ok_or_else(bad)?;
        return Ok((from_iso_cal(y, w, d)?, 8));
    }
    if raw.len() >= 7 && raw[4] == b'W' {
        let y = parse_ascii_digits(&raw[0..4]).ok_or_else(bad)?;
        let w = parse_ascii_digits(&raw[5..7]).ok_or_else(bad)?;
        return Ok((from_iso_cal(y, w, 1)?, 7));
    }
    if raw.len() >= 8 {
        let y = parse_ascii_digits(&raw[0..4]).ok_or_else(bad)?;
        let m = parse_ascii_digits(&raw[4..6]).ok_or_else(bad)?;
        let d = parse_ascii_digits(&raw[6..8]).ok_or_else(bad)?;
        validate_ymd(y, m, d)?;
        return Ok(((y, m, d), 8));
    }
    Err(bad())
}

fn parse_fraction_to_us(frac: &[u8], full: &str) -> Result<i64> {
    if frac.is_empty() || !frac.iter().all(u8::is_ascii_digit) {
        return Err(invalid_iso(full));
    }
    // Truncate or right-pad with zeros to 6 digits (CPython 3.11+ behavior).
    let mut us = 0i64;
    for i in 0..6 {
        let digit = if i < frac.len() {
            i64::from(frac[i] - b'0')
        } else {
            0
        };
        us = us * 10 + digit;
    }
    Ok(us)
}

/// Parse optional `[ :]SS[(.|,)frac]` suffix shared by ISO time and timezone
/// offsets after `MM` has been parsed.
fn parse_optional_sec_frac(
    bytes: &[u8],
    mut pos: usize,
    extended: bool,
    full: &str,
) -> Result<(i64, i64, usize)> {
    let bad = || invalid_iso(full);
    let mut ss = 0i64;
    let mut us = 0i64;
    if pos < bytes.len() {
        if extended {
            if bytes[pos] != b':' {
                return Err(bad());
            }
            pos += 1;
        }
        if pos + 2 > bytes.len() {
            return Err(bad());
        }
        ss = parse_ascii_digits(&bytes[pos..pos + 2]).ok_or_else(bad)?;
        pos += 2;
        if pos < bytes.len() {
            if bytes[pos] != b'.' && bytes[pos] != b',' {
                return Err(bad());
            }
            us = parse_fraction_to_us(&bytes[pos + 1..], full)?;
            pos = bytes.len();
        }
    }
    Ok((ss, us, pos))
}

fn parse_iso_tz(raw: &[u8], full: &str) -> Result<i64> {
    let bad = || invalid_iso(full);
    if raw == b"Z" || raw == b"z" {
        return Ok(0);
    }
    if raw.is_empty() {
        return Err(bad());
    }
    let sign = match raw[0] {
        b'+' => 1i64,
        b'-' => -1i64,
        _ => return Err(bad()),
    };
    let rest = &raw[1..];
    if rest.len() < 4 {
        return Err(bad());
    }
    let hh = parse_ascii_digits(&rest[0..2]).ok_or_else(bad)?;
    let (extended, pos) = if rest.get(2) == Some(&b':') {
        (true, 3usize)
    } else {
        (false, 2usize)
    };
    if pos + 2 > rest.len() {
        return Err(bad());
    }
    let mm = parse_ascii_digits(&rest[pos..pos + 2]).ok_or_else(bad)?;
    let (ss, us, end_pos) = parse_optional_sec_frac(rest, pos + 2, extended, full)?;
    if end_pos != rest.len() || hh > 23 || mm > 59 || ss > 59 {
        return Err(bad());
    }
    let total_us = ((hh * 3600 + mm * 60 + ss) * 1_000_000) + us;
    Ok(sign * total_us)
}

pub(super) fn parse_iso_time_bytes(
    raw: &[u8],
    full: &str,
) -> Result<(i64, i64, i64, i64, Option<i64>)> {
    let bad = || invalid_iso(full);
    let raw = if raw.first() == Some(&b'T') || raw.first() == Some(&b't') {
        &raw[1..]
    } else {
        raw
    };
    if raw.len() < 2 {
        return Err(bad());
    }
    // Split off timezone suffix (`Z`, `z`, `+...`, `-...`).
    let tz_pos = raw
        .iter()
        .position(|&b| matches!(b, b'Z' | b'z' | b'+' | b'-'));
    let (time_part, tz_us) = match tz_pos {
        Some(idx) => (&raw[..idx], Some(parse_iso_tz(&raw[idx..], full)?)),
        None => (raw, None),
    };
    if time_part.len() < 2 {
        return Err(bad());
    }
    let hour = parse_ascii_digits(&time_part[0..2]).ok_or_else(bad)?;
    let mut minute = 0i64;
    let mut second = 0i64;
    let mut us = 0i64;

    if time_part.len() > 2 {
        let extended = time_part[2] == b':';
        let pos = if extended { 3 } else { 2 };
        if pos + 2 > time_part.len() {
            return Err(bad());
        }
        minute = parse_ascii_digits(&time_part[pos..pos + 2]).ok_or_else(bad)?;
        let (s, u, _) = parse_optional_sec_frac(time_part, pos + 2, extended, full)?;
        second = s;
        us = u;
    }

    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return Err(bad());
    }
    Ok((hour, minute, second, us, tz_us))
}

pub(super) fn parse_iso_datetime_bytes(raw: &[u8], full: &str) -> Result<IsoDateTimeParts> {
    let bad = || invalid_iso(full);
    let ((y, mo, d), date_len) = parse_iso_date_prefix(raw, full)?;
    if date_len == raw.len() {
        return Ok((y, mo, d, 0, 0, 0, 0, None));
    }
    // Single-character separator between date and time (e.g. 'T', 't', ' ', '_').
    let sep_rest = &raw[date_len..];
    if sep_rest.len() < 3 {
        return Err(bad());
    }
    let sep_str = std::str::from_utf8(sep_rest).map_err(|_| bad())?;
    let sep_len = sep_str.chars().next().ok_or_else(bad)?.len_utf8();
    let time_bytes = &sep_rest[sep_len..];
    if time_bytes.first() == Some(&b'T') || time_bytes.first() == Some(&b't') {
        return Err(bad());
    }
    let (h, mi, se, us, tz_us) = parse_iso_time_bytes(time_bytes, full)?;
    Ok((y, mo, d, h, mi, se, us, tz_us))
}
