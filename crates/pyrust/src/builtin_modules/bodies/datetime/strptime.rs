//! `strptime` format-driven parsing implementation for `datetime`.

use crate::error::{PyError, Result};

use super::calendar::{
    FULL_MONTHS, FULL_WEEKDAYS, MAX_ORDINAL, SHORT_MONTHS, SHORT_WEEKDAYS, from_iso_cal,
    is_leap_year, ord_to_ymd, validate_ymd, weekday_from_ord, ymd_to_ord,
};
use super::iso_parse::parse_ascii_digits;

pub(super) type StrptimeResult = (
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    Option<i64>,
    Option<String>,
);

fn consume_digits(bytes: &[u8], pos: &mut usize, min_len: usize, max_len: usize) -> Option<i64> {
    let start = *pos;
    let limit = (start + max_len).min(bytes.len());
    while *pos < limit && bytes[*pos].is_ascii_digit() {
        *pos += 1;
    }
    if *pos - start < min_len {
        return None;
    }
    parse_ascii_digits(&bytes[start..*pos])
}

fn consume_ranged_digits(
    bytes: &[u8],
    pos: &mut usize,
    min_len: usize,
    max_len: usize,
    lo: i64,
    hi: i64,
) -> Option<i64> {
    let v = consume_digits(bytes, pos, min_len, max_len)?;
    (lo..=hi).contains(&v).then_some(v)
}

/// Consume 1..=6 fractional digits and right-pad with zeros to microseconds.
fn consume_fraction_us(bytes: &[u8], pos: &mut usize) -> Option<i64> {
    let start = *pos;
    let limit = (start + 6).min(bytes.len());
    while *pos < limit && bytes[*pos].is_ascii_digit() {
        *pos += 1;
    }
    let len = *pos - start;
    if len == 0 {
        return None;
    }
    let mut val = parse_ascii_digits(&bytes[start..*pos])?;
    for _ in len..6 {
        val *= 10;
    }
    Some(val)
}

fn match_ci_table(bytes: &[u8], pos: &mut usize, table: &[&str]) -> Option<usize> {
    let rest = &bytes[*pos..];
    for (idx, &entry) in table.iter().enumerate() {
        let eb = entry.as_bytes();
        if rest.len() >= eb.len() && rest[..eb.len()].eq_ignore_ascii_case(eb) {
            *pos += eb.len();
            return Some(idx);
        }
    }
    None
}

fn parse_strptime_z(tb: &[u8], ti: &mut usize, require_colon: bool) -> Option<i64> {
    if *ti >= tb.len() {
        return None;
    }
    if tb[*ti] == b'Z' || tb[*ti] == b'z' {
        *ti += 1;
        return Some(0);
    }
    let sign = match tb[*ti] {
        b'+' => 1i64,
        b'-' => -1i64,
        _ => return None,
    };
    *ti += 1;
    let hh = consume_digits(tb, ti, 2, 2)?;
    let has_colon = tb.get(*ti) == Some(&b':');
    if require_colon && !has_colon {
        return None;
    }
    if has_colon {
        *ti += 1;
    }
    let mm = consume_digits(tb, ti, 2, 2)?;
    let mut ss = 0i64;
    let mut us = 0i64;
    let has_seconds = if has_colon && tb.get(*ti) == Some(&b':') {
        *ti += 1;
        true
    } else {
        !has_colon
            && tb.get(*ti).is_some_and(u8::is_ascii_digit)
            && tb.get(*ti + 1).is_some_and(u8::is_ascii_digit)
    };
    if has_seconds {
        ss = consume_digits(tb, ti, 2, 2)?;
        if tb.get(*ti) == Some(&b'.') {
            *ti += 1;
            us = consume_fraction_us(tb, ti)?;
        }
    }
    if hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    Some(sign * ((hh * 3600 + mm * 60 + ss) * 1_000_000 + us))
}

pub(super) fn strptime_parse(text: &str, fmt: &str) -> Result<StrptimeResult> {
    let mismatch = || {
        PyError::named(
            "ValueError",
            format!("time data '{text}' does not match format '{fmt}'"),
        )
    };
    let tb = text.as_bytes();
    let fb = fmt.as_bytes();
    let mut ti = 0usize;
    let mut fi = 0usize;

    let mut year: Option<i64> = None;
    let mut month: Option<i64> = None;
    let mut day: Option<i64> = None;
    let mut hour: Option<i64> = None;
    let mut hour12: Option<i64> = None;
    let mut is_pm: Option<bool> = None;
    let mut minute: Option<i64> = None;
    let mut second: Option<i64> = None;
    let mut us: i64 = 0;
    let mut julian: Option<i64> = None;
    let mut weekday_mon0: Option<i64> = None;
    let mut week_num: Option<i64> = None;
    let mut week_starts_mon = false;
    let mut iso_year: Option<i64> = None;
    let mut iso_week: Option<i64> = None;
    let mut tz_offset_us: Option<i64> = None;
    let mut tz_name: Option<String> = None;

    while fi < fb.len() {
        let fc = fb[fi];
        if fc.is_ascii_whitespace() {
            while fi < fb.len() && fb[fi].is_ascii_whitespace() {
                fi += 1;
            }
            while ti < tb.len() && tb[ti].is_ascii_whitespace() {
                ti += 1;
            }
            continue;
        }
        if fc != b'%' {
            if ti >= tb.len() || tb[ti] != fc {
                return Err(mismatch());
            }
            fi += 1;
            ti += 1;
            continue;
        }
        fi += 1;
        if fi >= fb.len() {
            return Err(PyError::named(
                "ValueError",
                "stray % in format".to_string(),
            ));
        }
        // Handle `%:z`.
        if fb[fi] == b':' && fb.get(fi + 1) == Some(&b'z') {
            fi += 2;
            let off = parse_strptime_z(tb, &mut ti, true).ok_or_else(mismatch)?;
            tz_offset_us = Some(off);
            continue;
        }
        let spec = fb[fi];
        fi += 1;
        match spec {
            b'%' => {
                if ti >= tb.len() || tb[ti] != b'%' {
                    return Err(mismatch());
                }
                ti += 1;
            }
            b'Y' => year = Some(consume_digits(tb, &mut ti, 4, 4).ok_or_else(mismatch)?),
            b'y' => {
                let v = consume_digits(tb, &mut ti, 2, 2).ok_or_else(mismatch)?;
                year = Some(if v <= 68 { 2000 + v } else { 1900 + v });
            }
            b'm' => {
                month = Some(consume_ranged_digits(tb, &mut ti, 1, 2, 1, 12).ok_or_else(mismatch)?);
            }
            b'd' => {
                day = Some(consume_ranged_digits(tb, &mut ti, 1, 2, 1, 31).ok_or_else(mismatch)?);
            }
            b'H' => {
                hour = Some(consume_ranged_digits(tb, &mut ti, 1, 2, 0, 23).ok_or_else(mismatch)?);
            }
            b'I' => {
                hour12 =
                    Some(consume_ranged_digits(tb, &mut ti, 1, 2, 1, 12).ok_or_else(mismatch)?);
            }
            b'p' => {
                if ti + 2 <= tb.len() && tb[ti..ti + 2].eq_ignore_ascii_case(b"AM") {
                    is_pm = Some(false);
                    ti += 2;
                } else if ti + 2 <= tb.len() && tb[ti..ti + 2].eq_ignore_ascii_case(b"PM") {
                    is_pm = Some(true);
                    ti += 2;
                } else {
                    return Err(mismatch());
                }
            }
            b'M' => {
                minute =
                    Some(consume_ranged_digits(tb, &mut ti, 1, 2, 0, 59).ok_or_else(mismatch)?);
            }
            b'S' => {
                second =
                    Some(consume_ranged_digits(tb, &mut ti, 1, 2, 0, 59).ok_or_else(mismatch)?);
            }
            b'f' => us = consume_fraction_us(tb, &mut ti).ok_or_else(mismatch)?,
            b'j' => {
                julian =
                    Some(consume_ranged_digits(tb, &mut ti, 1, 3, 1, 366).ok_or_else(mismatch)?);
            }
            b'a' => {
                let idx = match_ci_table(tb, &mut ti, &SHORT_WEEKDAYS).ok_or_else(mismatch)?;
                weekday_mon0 = Some(idx as i64);
            }
            b'A' => {
                let idx = match_ci_table(tb, &mut ti, &FULL_WEEKDAYS).ok_or_else(mismatch)?;
                weekday_mon0 = Some(idx as i64);
            }
            b'b' | b'h' => {
                let idx = match_ci_table(tb, &mut ti, &SHORT_MONTHS).ok_or_else(mismatch)?;
                month = Some((idx + 1) as i64);
            }
            b'B' => {
                let idx = match_ci_table(tb, &mut ti, &FULL_MONTHS).ok_or_else(mismatch)?;
                month = Some((idx + 1) as i64);
            }
            b'w' => {
                let v = consume_ranged_digits(tb, &mut ti, 1, 1, 0, 6).ok_or_else(mismatch)?;
                weekday_mon0 = Some((v + 6) % 7);
            }
            b'u' => {
                let v = consume_ranged_digits(tb, &mut ti, 1, 1, 1, 7).ok_or_else(mismatch)?;
                weekday_mon0 = Some(v - 1);
            }
            b'U' => {
                week_num =
                    Some(consume_ranged_digits(tb, &mut ti, 1, 2, 0, 53).ok_or_else(mismatch)?);
                week_starts_mon = false;
            }
            b'W' => {
                week_num =
                    Some(consume_ranged_digits(tb, &mut ti, 1, 2, 0, 53).ok_or_else(mismatch)?);
                week_starts_mon = true;
            }
            b'G' => iso_year = Some(consume_digits(tb, &mut ti, 4, 4).ok_or_else(mismatch)?),
            b'V' => {
                iso_week =
                    Some(consume_ranged_digits(tb, &mut ti, 1, 2, 1, 53).ok_or_else(mismatch)?);
            }
            b'z' => {
                tz_offset_us = Some(parse_strptime_z(tb, &mut ti, false).ok_or_else(mismatch)?);
            }
            b'Z' => {
                if ti + 3 <= tb.len()
                    && (tb[ti..ti + 3].eq_ignore_ascii_case(b"UTC")
                        || tb[ti..ti + 3].eq_ignore_ascii_case(b"GMT"))
                {
                    tz_name = Some(
                        std::str::from_utf8(&tb[ti..ti + 3])
                            .unwrap_or("UTC")
                            .to_string(),
                    );
                    ti += 3;
                } else {
                    return Err(mismatch());
                }
            }
            other => {
                return Err(PyError::named(
                    "ValueError",
                    format!("'{}' is a bad directive in format '{fmt}'", other as char),
                ));
            }
        }
    }

    if ti < tb.len() {
        let remaining = &text[ti..];
        return Err(PyError::named(
            "ValueError",
            format!("unconverted data remains: {remaining}"),
        ));
    }

    let (y, mo, d) = if let (Some(iy), Some(iw)) = (iso_year, iso_week) {
        let id = weekday_mon0.map(|w| w + 1).ok_or_else(|| {
            PyError::named(
                "ValueError",
                "ISO year directive '%G' must be used with the ISO week directive '%V' and a weekday directive ('%A', '%a', '%w', or '%u').".to_string(),
            )
        })?;
        from_iso_cal(iy, iw, id)?
    } else if let Some(j) = julian {
        let y = year.unwrap_or(1900);
        let max_j = if is_leap_year(y) { 366 } else { 365 };
        if j > max_j {
            return Err(mismatch());
        }
        ord_to_ymd(ymd_to_ord(y, 1, 1) + j - 1)
    } else if let (Some(wn), Some(wd_mon0)) = (week_num, weekday_mon0) {
        let y = year.unwrap_or(1900);
        let jan1 = ymd_to_ord(y, 1, 1);
        let jan1_wday = if week_starts_mon {
            weekday_from_ord(jan1)
        } else {
            (weekday_from_ord(jan1) + 1) % 7
        };
        let target_wday = if week_starts_mon {
            wd_mon0
        } else {
            (wd_mon0 + 1) % 7
        };
        let first_week_offset = (7 - jan1_wday) % 7;
        let days_offset = if wn == 0 {
            target_wday - jan1_wday
        } else {
            first_week_offset + (wn - 1) * 7 + target_wday
        };
        let ord = jan1 + days_offset;
        if !(1..=MAX_ORDINAL).contains(&ord) {
            return Err(mismatch());
        }
        ord_to_ymd(ord)
    } else {
        let y = year.unwrap_or(1900);
        let mo = month.unwrap_or(1);
        let d = day.unwrap_or(1);
        validate_ymd(y, mo, d)?;
        (y, mo, d)
    };

    let h = if let Some(hv) = hour {
        hv
    } else if let Some(h12) = hour12 {
        match is_pm {
            Some(true) => {
                if h12 == 12 {
                    12
                } else {
                    h12 + 12
                }
            }
            _ => {
                if h12 == 12 {
                    0
                } else {
                    h12
                }
            }
        }
    } else {
        0
    };

    Ok((
        y,
        mo,
        d,
        h,
        minute.unwrap_or(0),
        second.unwrap_or(0),
        us,
        tz_offset_us,
        tz_name,
    ))
}
