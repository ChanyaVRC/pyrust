// `datetime` module — body for the `datetime @inject` entry in
// `pyrust_builtin_modules!` (issue #2805).
//
// Core Gregorian calendar arithmetic (O(1) civil/ordinal conversion), ISO-8601
// parsing, `strftime` formatting, and `strptime` parsing live in Rust here.
// The six Python types (`timedelta`, `tzinfo`, `timezone`, `date`, `time`,
// `datetime`) plus `UTC`, `MINYEAR`, and `MAXYEAR` live in `datetime_py.py`
// and are injected onto the module by `inject_python_members`.  Internal Rust
// helpers are removed from `datetime.__dict__` after wiring so `dir(datetime)`
// and `datetime.__all__` match CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/datetime.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the public `datetime` classes and constants.
const DATETIME_PY_SOURCE: &str = include_str!("datetime_py.py");

/// Internal native helpers seeded into the injected exec namespace and removed
/// from `datetime.__dict__` after injection.
const DATETIME_NATIVE_HELPERS: [&str; 9] = [
    "_ymd_to_ordinal",
    "_ordinal_to_ymd",
    "_iso_calendar",
    "_from_iso_calendar",
    "_parse_iso_date",
    "_parse_iso_time",
    "_parse_iso_datetime",
    "_format_strftime",
    "_parse_strptime",
];

/// Public names exported from `DATETIME_PY_SOURCE` onto the `datetime` module.
const DATETIME_PY_EXPORTS: [&str; 9] = [
    "MINYEAR",
    "MAXYEAR",
    "UTC",
    "date",
    "time",
    "datetime",
    "timedelta",
    "tzinfo",
    "timezone",
];

const MIN_YEAR: i64 = 1;
const MAX_YEAR: i64 = 9999;
const MAX_ORDINAL: i64 = 3_652_059;
const UNIX_EPOCH_ORDINAL: i64 = 719_163;

const SHORT_WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const FULL_WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const SHORT_MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const FULL_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Execute `DATETIME_PY_SOURCE` once and publish its public names onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in DATETIME_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    let deps: Vec<(&str, Value)> = [
        ("_time", "time"),
        ("_math", "math"),
        ("_operator", "operator"),
    ]
    .into_iter()
    .map(|(alias, mod_name)| interp.load_module(mod_name).map(|m| (alias, m)))
    .collect::<Result<_>>()?;
    ns.dict_with_mut(|d| {
        for (alias, m) in &deps {
            d.insert(PyKey::str_from(alias), m.clone());
        }
    });
    interp.exec_source(DATETIME_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("datetime: exec namespace not a dict".into()))?;
    for name in DATETIME_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("datetime"));
                class.error_name = match name {
                    "date" => Some("datetime.date"),
                    "time" => Some("datetime.time"),
                    "datetime" => Some("datetime.datetime"),
                    "timedelta" => Some("datetime.timedelta"),
                    "tzinfo" => Some("datetime.tzinfo"),
                    "timezone" => Some("datetime.timezone"),
                    _ => None,
                };
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Gregorian calendar math ──────────────────────────────────────────────────

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn month_length(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn validate_ymd(year: i64, month: i64, day: i64) -> Result<()> {
    if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
        return Err(PyError::named(
            "ValueError",
            format!("year {year} is out of range"),
        ));
    }
    if !(1..=12).contains(&month) {
        return Err(PyError::named(
            "ValueError",
            "month must be in 1..12".to_string(),
        ));
    }
    let dim = month_length(year, month);
    if !(1..=dim).contains(&day) {
        return Err(PyError::named(
            "ValueError",
            "day is out of range for month".to_string(),
        ));
    }
    Ok(())
}

/// Proleptic Gregorian ordinal where `0001-01-01 == 1`.
fn ymd_to_ord(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m_adj = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m_adj + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468 + UNIX_EPOCH_ORDINAL
}

/// Inverse of [`ymd_to_ord`] for `1 <= ord <= 3_652_059`.
fn ord_to_ymd(ord: i64) -> (i64, i64, i64) {
    let z = ord - UNIX_EPOCH_ORDINAL + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

fn day_of_year(year: i64, month: i64, day: i64) -> i64 {
    ymd_to_ord(year, month, day) - ymd_to_ord(year, 1, 1) + 1
}

/// Monday = 0 .. Sunday = 6.
fn weekday_from_ord(ord: i64) -> i64 {
    (ord - 1).rem_euclid(7)
}

/// Ordinal of Monday of ISO week 1 for `year`.
fn iso_week1_monday(year: i64) -> i64 {
    let jan4 = ymd_to_ord(year, 1, 4);
    jan4 - weekday_from_ord(jan4)
}

fn calc_iso_calendar(year: i64, month: i64, day: i64) -> (i64, i64, i64) {
    let ord = ymd_to_ord(year, month, day);
    let mut iso_year = year;
    let mut week1 = iso_week1_monday(iso_year);
    if ord < week1 {
        iso_year -= 1;
        week1 = iso_week1_monday(iso_year);
    } else if iso_year < MAX_YEAR {
        let next_week1 = iso_week1_monday(iso_year + 1);
        if ord >= next_week1 {
            iso_year += 1;
            week1 = next_week1;
        }
    }
    let delta = ord - week1;
    let iso_week = delta / 7 + 1;
    let iso_day = delta % 7 + 1;
    (iso_year, iso_week, iso_day)
}

fn from_iso_cal(iso_year: i64, iso_week: i64, iso_day: i64) -> Result<(i64, i64, i64)> {
    if !(MIN_YEAR..=MAX_YEAR).contains(&iso_year) {
        return Err(PyError::named(
            "ValueError",
            format!("Year is out of range: {iso_year}"),
        ));
    }
    if !(1..=7).contains(&iso_day) {
        return Err(PyError::named(
            "ValueError",
            format!("Invalid day: {iso_day} (range is [1, 7])"),
        ));
    }
    let week1 = iso_week1_monday(iso_year);
    let max_weeks = if iso_year == MAX_YEAR {
        52
    } else {
        (iso_week1_monday(iso_year + 1) - week1) / 7
    };
    if !(1..=max_weeks).contains(&iso_week) {
        return Err(PyError::named(
            "ValueError",
            format!("Invalid week: {iso_week}"),
        ));
    }
    let ord = week1 + (iso_week - 1) * 7 + (iso_day - 1);
    if !(1..=MAX_ORDINAL).contains(&ord) {
        return Err(PyError::named(
            "ValueError",
            format!("Year is out of range: {iso_year}"),
        ));
    }
    Ok(ord_to_ymd(ord))
}

// ── ISO-8601 parsing ─────────────────────────────────────────────────────────

fn parse_ascii_digits(bytes: &[u8]) -> Option<i64> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut acc = 0i64;
    for &b in bytes {
        acc = acc * 10 + i64::from(b - b'0');
    }
    Some(acc)
}

fn parse_iso_date_bytes(raw: &[u8], full: &str) -> Result<(i64, i64, i64)> {
    let bad = || PyError::named("ValueError", format!("Invalid isoformat string: '{full}'"));
    let (ymd, consumed) = parse_iso_date_prefix(raw, full)?;
    if consumed != raw.len() {
        return Err(bad());
    }
    Ok(ymd)
}

fn parse_iso_date_prefix(raw: &[u8], full: &str) -> Result<((i64, i64, i64), usize)> {
    let bad = || PyError::named("ValueError", format!("Invalid isoformat string: '{full}'"));
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
    let bad = || PyError::named("ValueError", format!("Invalid isoformat string: '{full}'"));
    if frac.is_empty() || !frac.iter().all(u8::is_ascii_digit) {
        return Err(bad());
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

fn parse_iso_tz(raw: &[u8], full: &str) -> Result<i64> {
    let bad = || PyError::named("ValueError", format!("Invalid isoformat string: '{full}'"));
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
    let (extended, mut pos) = if rest.get(2) == Some(&b':') {
        (true, 3usize)
    } else {
        (false, 2usize)
    };
    if pos + 2 > rest.len() {
        return Err(bad());
    }
    let mm = parse_ascii_digits(&rest[pos..pos + 2]).ok_or_else(bad)?;
    pos += 2;
    let mut ss = 0i64;
    let mut us = 0i64;
    if pos < rest.len() {
        if extended {
            if rest[pos] != b':' {
                return Err(bad());
            }
            pos += 1;
        }
        if pos + 2 > rest.len() {
            return Err(bad());
        }
        ss = parse_ascii_digits(&rest[pos..pos + 2]).ok_or_else(bad)?;
        pos += 2;
        if pos < rest.len() {
            if rest[pos] != b'.' && rest[pos] != b',' {
                return Err(bad());
            }
            us = parse_fraction_to_us(&rest[pos + 1..], full)?;
            pos = rest.len();
        }
    }
    if pos != rest.len() || hh > 23 || mm > 59 || ss > 59 {
        return Err(bad());
    }
    let total_us = ((hh * 3600 + mm * 60 + ss) * 1_000_000) + us;
    Ok(sign * total_us)
}

fn parse_iso_time_bytes(raw: &[u8], full: &str) -> Result<(i64, i64, i64, i64, Option<i64>)> {
    let bad = || PyError::named("ValueError", format!("Invalid isoformat string: '{full}'"));
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
        let mut pos = if extended { 3 } else { 2 };
        if pos + 2 > time_part.len() {
            return Err(bad());
        }
        minute = parse_ascii_digits(&time_part[pos..pos + 2]).ok_or_else(bad)?;
        pos += 2;
        if pos < time_part.len() {
            if extended {
                if time_part[pos] != b':' {
                    return Err(bad());
                }
                pos += 1;
            }
            if pos + 2 > time_part.len() {
                return Err(bad());
            }
            second = parse_ascii_digits(&time_part[pos..pos + 2]).ok_or_else(bad)?;
            pos += 2;
            if pos < time_part.len() {
                if time_part[pos] != b'.' && time_part[pos] != b',' {
                    return Err(bad());
                }
                us = parse_fraction_to_us(&time_part[pos + 1..], full)?;
            }
        }
    }

    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) || !(0..=59).contains(&second) {
        return Err(PyError::named(
            "ValueError",
            format!("Invalid isoformat string: '{full}'"),
        ));
    }
    Ok((hour, minute, second, us, tz_us))
}

type IsoDateTimeParts = (i64, i64, i64, i64, i64, i64, i64, Option<i64>);

fn parse_iso_datetime_bytes(raw: &[u8], full: &str) -> Result<IsoDateTimeParts> {
    let bad = || PyError::named("ValueError", format!("Invalid isoformat string: '{full}'"));
    let ((y, mo, d), date_len) = parse_iso_date_prefix(raw, full)?;
    if date_len == raw.len() {
        return Ok((y, mo, d, 0, 0, 0, 0, None));
    }
    // Single-character separator between date and time (e.g. 'T', 't', ' ', '_').
    let sep_rest = &raw[date_len..];
    if sep_rest.len() < 3 {
        return Err(bad());
    }
    // Advance past one UTF-8 character separator.
    let sep_str = std::str::from_utf8(sep_rest).map_err(|_| bad())?;
    let sep_len = sep_str.chars().next().ok_or_else(bad)?.len_utf8();
    let time_bytes = &sep_rest[sep_len..];
    if time_bytes.first() == Some(&b'T') || time_bytes.first() == Some(&b't') {
        return Err(bad());
    }
    let (h, mi, se, us, tz_us) = parse_iso_time_bytes(time_bytes, full)?;
    Ok((y, mo, d, h, mi, se, us, tz_us))
}

// ── strftime implementation ──────────────────────────────────────────────────

fn format_offset(total_us: i64, colon: bool) -> String {
    let sign = if total_us < 0 { '-' } else { '+' };
    let abs_us = total_us.abs();
    let total_secs = abs_us / 1_000_000;
    let rem_us = abs_us % 1_000_000;
    let hh = total_secs / 3600;
    let mm = (total_secs % 3600) / 60;
    let ss = total_secs % 60;
    let sep = if colon { ":" } else { "" };
    if rem_us != 0 {
        format!("{sign}{hh:02}{sep}{mm:02}{sep}{ss:02}.{rem_us:06}")
    } else if ss != 0 {
        format!("{sign}{hh:02}{sep}{mm:02}{sep}{ss:02}")
    } else {
        format!("{sign}{hh:02}{sep}{mm:02}")
    }
}

#[allow(clippy::too_many_arguments)]
fn strftime_format(
    fmt: &str,
    y: i64,
    mo: i64,
    d: i64,
    h: i64,
    mi: i64,
    se: i64,
    us: i64,
    tz_offset_us: Option<i64>,
    tz_name: Option<&str>,
) -> String {
    let ord = ymd_to_ord(y, mo, d);
    let wday_mon0 = weekday_from_ord(ord); // Mon=0..Sun=6
    let wday_sun0 = (wday_mon0 + 1) % 7; // Sun=0..Sat=6
    let yday = day_of_year(y, mo, d); // 1..366
    let (iso_y, iso_w, iso_d) = calc_iso_calendar(y, mo, d);

    let mut out = String::with_capacity(fmt.len() + 16);
    let mut chars = fmt.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        let Some(spec) = chars.next() else {
            out.push('%');
            break;
        };
        if spec == ':' && chars.peek() == Some(&'z') {
            chars.next();
            if let Some(off) = tz_offset_us {
                out.push_str(&format_offset(off, true));
            }
            continue;
        }
        match spec {
            '%' => out.push('%'),
            'Y' => out.push_str(&format!("{y:04}")),
            'y' => out.push_str(&format!("{:02}", y % 100)),
            'm' => out.push_str(&format!("{mo:02}")),
            'd' => out.push_str(&format!("{d:02}")),
            'H' => out.push_str(&format!("{h:02}")),
            'I' => {
                let h12 = match h % 12 {
                    0 => 12,
                    v => v,
                };
                out.push_str(&format!("{h12:02}"));
            }
            'p' => out.push_str(if h < 12 { "AM" } else { "PM" }),
            'M' => out.push_str(&format!("{mi:02}")),
            'S' => out.push_str(&format!("{se:02}")),
            'f' => out.push_str(&format!("{us:06}")),
            'j' => out.push_str(&format!("{yday:03}")),
            'a' => out.push_str(SHORT_WEEKDAYS[wday_mon0 as usize]),
            'A' => out.push_str(FULL_WEEKDAYS[wday_mon0 as usize]),
            'w' => out.push_str(&wday_sun0.to_string()),
            'u' => out.push_str(&iso_d.to_string()),
            'b' | 'h' => out.push_str(SHORT_MONTHS[(mo - 1) as usize]),
            'B' => out.push_str(FULL_MONTHS[(mo - 1) as usize]),
            'U' => {
                // Sunday-based week number (00..53).
                let wk = (yday + 6 - wday_sun0) / 7;
                out.push_str(&format!("{wk:02}"));
            }
            'W' => {
                // Monday-based week number (00..53).
                let wk = (yday + 6 - wday_mon0) / 7;
                out.push_str(&format!("{wk:02}"));
            }
            'G' => out.push_str(&format!("{iso_y:04}")),
            'V' => out.push_str(&format!("{iso_w:02}")),
            'z' => {
                if let Some(off) = tz_offset_us {
                    out.push_str(&format_offset(off, false));
                }
            }
            'Z' => {
                if let Some(name) = tz_name {
                    out.push_str(name);
                }
            }
            'c' => {
                out.push_str(&format!(
                    "{} {} {:2} {:02}:{:02}:{:02} {:04}",
                    SHORT_WEEKDAYS[wday_mon0 as usize],
                    SHORT_MONTHS[(mo - 1) as usize],
                    d,
                    h,
                    mi,
                    se,
                    y
                ));
            }
            'x' => out.push_str(&format!("{mo:02}/{d:02}/{:02}", y % 100)),
            'X' => out.push_str(&format!("{h:02}:{mi:02}:{se:02}")),
            other => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
}

// ── strptime implementation ──────────────────────────────────────────────────

fn consume_digits(bytes: &[u8], pos: &mut usize, min_len: usize, max_len: usize) -> Option<i64> {
    let start = *pos;
    let limit = (start + max_len).min(bytes.len());
    while *pos < limit && bytes[*pos].is_ascii_digit() {
        *pos += 1;
    }
    let len = *pos - start;
    if len < min_len {
        return None;
    }
    parse_ascii_digits(&bytes[start..*pos])
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

type StrptimeResult = (
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

fn strptime_parse(text: &str, fmt: &str) -> Result<StrptimeResult> {
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
            b'Y' => {
                let v = consume_digits(tb, &mut ti, 4, 4).ok_or_else(mismatch)?;
                year = Some(v);
            }
            b'y' => {
                let v = consume_digits(tb, &mut ti, 2, 2).ok_or_else(mismatch)?;
                year = Some(if v <= 68 { 2000 + v } else { 1900 + v });
            }
            b'm' => {
                let v = consume_digits(tb, &mut ti, 1, 2).ok_or_else(mismatch)?;
                if !(1..=12).contains(&v) {
                    return Err(mismatch());
                }
                month = Some(v);
            }
            b'd' => {
                let v = consume_digits(tb, &mut ti, 1, 2).ok_or_else(mismatch)?;
                if !(1..=31).contains(&v) {
                    return Err(mismatch());
                }
                day = Some(v);
            }
            b'H' => {
                let v = consume_digits(tb, &mut ti, 1, 2).ok_or_else(mismatch)?;
                if !(0..=23).contains(&v) {
                    return Err(mismatch());
                }
                hour = Some(v);
            }
            b'I' => {
                let v = consume_digits(tb, &mut ti, 1, 2).ok_or_else(mismatch)?;
                if !(1..=12).contains(&v) {
                    return Err(mismatch());
                }
                hour12 = Some(v);
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
                let v = consume_digits(tb, &mut ti, 1, 2).ok_or_else(mismatch)?;
                if !(0..=59).contains(&v) {
                    return Err(mismatch());
                }
                minute = Some(v);
            }
            b'S' => {
                let v = consume_digits(tb, &mut ti, 1, 2).ok_or_else(mismatch)?;
                if !(0..=59).contains(&v) {
                    return Err(mismatch());
                }
                second = Some(v);
            }
            b'f' => {
                let start = ti;
                let limit = (start + 6).min(tb.len());
                while ti < limit && tb[ti].is_ascii_digit() {
                    ti += 1;
                }
                let len = ti - start;
                if len == 0 {
                    return Err(mismatch());
                }
                let mut val = parse_ascii_digits(&tb[start..ti]).ok_or_else(mismatch)?;
                for _ in len..6 {
                    val *= 10;
                }
                us = val;
            }
            b'j' => {
                let v = consume_digits(tb, &mut ti, 1, 3).ok_or_else(mismatch)?;
                if !(1..=366).contains(&v) {
                    return Err(mismatch());
                }
                julian = Some(v);
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
                let v = consume_digits(tb, &mut ti, 1, 1).ok_or_else(mismatch)?;
                if !(0..=6).contains(&v) {
                    return Err(mismatch());
                }
                weekday_mon0 = Some((v + 6) % 7);
            }
            b'u' => {
                let v = consume_digits(tb, &mut ti, 1, 1).ok_or_else(mismatch)?;
                if !(1..=7).contains(&v) {
                    return Err(mismatch());
                }
                weekday_mon0 = Some(v - 1);
            }
            b'U' => {
                let v = consume_digits(tb, &mut ti, 1, 2).ok_or_else(mismatch)?;
                if !(0..=53).contains(&v) {
                    return Err(mismatch());
                }
                week_num = Some(v);
                week_starts_mon = false;
            }
            b'W' => {
                let v = consume_digits(tb, &mut ti, 1, 2).ok_or_else(mismatch)?;
                if !(0..=53).contains(&v) {
                    return Err(mismatch());
                }
                week_num = Some(v);
                week_starts_mon = true;
            }
            b'G' => {
                let v = consume_digits(tb, &mut ti, 4, 4).ok_or_else(mismatch)?;
                iso_year = Some(v);
            }
            b'V' => {
                let v = consume_digits(tb, &mut ti, 1, 2).ok_or_else(mismatch)?;
                if !(1..=53).contains(&v) {
                    return Err(mismatch());
                }
                iso_week = Some(v);
            }
            b'z' => {
                let off = parse_strptime_z(tb, &mut ti, false).ok_or_else(mismatch)?;
                tz_offset_us = Some(off);
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
        let ord = ymd_to_ord(y, 1, 1) + j - 1;
        ord_to_ymd(ord)
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
    if has_colon && tb.get(*ti) == Some(&b':') {
        *ti += 1;
        ss = consume_digits(tb, ti, 2, 2)?;
        if tb.get(*ti) == Some(&b'.') {
            *ti += 1;
            let start = *ti;
            let limit = (start + 6).min(tb.len());
            while *ti < limit && tb[*ti].is_ascii_digit() {
                *ti += 1;
            }
            if *ti == start {
                return None;
            }
            let mut val = parse_ascii_digits(&tb[start..*ti])?;
            for _ in (*ti - start)..6 {
                val *= 10;
            }
            us = val;
        }
    } else if !has_colon
        && tb.get(*ti).is_some_and(u8::is_ascii_digit)
        && tb.get(*ti + 1).is_some_and(u8::is_ascii_digit)
    {
        ss = consume_digits(tb, ti, 2, 2)?;
        if tb.get(*ti) == Some(&b'.') {
            *ti += 1;
            let start = *ti;
            let limit = (start + 6).min(tb.len());
            while *ti < limit && tb[*ti].is_ascii_digit() {
                *ti += 1;
            }
            if *ti == start {
                return None;
            }
            let mut val = parse_ascii_digits(&tb[start..*ti])?;
            for _ in (*ti - start)..6 {
                val *= 10;
            }
            us = val;
        }
    }
    if hh > 23 || mm > 59 || ss > 59 {
        return None;
    }
    Some(sign * ((hh * 3600 + mm * 60 + ss) * 1_000_000 + us))
}

fn expect_int_arg(val: &Value, label: &str) -> Result<i64> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(i64::from(b)),
        _ => Err(PyError::named(
            "TypeError",
            format!("'{label}' must be an integer"),
        )),
    }
}

fn expect_str_arg<'a>(val: &'a Value, label: &str) -> Result<&'a str> {
    match val.kind() {
        ValueKind::Str(s) => Ok(s),
        _ => Err(PyError::named(
            "TypeError",
            format!("{label}: argument must be str"),
        )),
    }
}

pyrust_module! {
    fn _ymd_to_ordinal(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let y = expect_int_arg(&args[0].value, "year")?;
        let m = expect_int_arg(&args[1].value, "month")?;
        let d = expect_int_arg(&args[2].value, "day")?;
        validate_ymd(y, m, d)?;
        Ok(Value::int(ymd_to_ord(y, m, d)))
    }

    fn _ordinal_to_ymd(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let ord = expect_int_arg(&args[0].value, "ordinal")?;
        if !(1..=MAX_ORDINAL).contains(&ord) {
            return Err(PyError::named(
                "ValueError",
                format!("ordinal must be >= 1 and <= {MAX_ORDINAL}"),
            ));
        }
        let (y, m, d) = ord_to_ymd(ord);
        Ok(Value::tuple(vec![Value::int(y), Value::int(m), Value::int(d)]))
    }

    fn _iso_calendar(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let y = expect_int_arg(&args[0].value, "year")?;
        let m = expect_int_arg(&args[1].value, "month")?;
        let d = expect_int_arg(&args[2].value, "day")?;
        let (iy, iw, id) = calc_iso_calendar(y, m, d);
        Ok(Value::tuple(vec![Value::int(iy), Value::int(iw), Value::int(id)]))
    }

    fn _from_iso_calendar(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let iy = expect_int_arg(&args[0].value, "year")?;
        let iw = expect_int_arg(&args[1].value, "week")?;
        let id = expect_int_arg(&args[2].value, "day")?;
        let (y, m, d) = from_iso_cal(iy, iw, id)?;
        Ok(Value::tuple(vec![Value::int(y), Value::int(m), Value::int(d)]))
    }

    fn _parse_iso_date(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let s = expect_str_arg(&args[0].value, "fromisoformat")?;
        let (y, m, d) = parse_iso_date_bytes(s.as_bytes(), s)?;
        Ok(Value::tuple(vec![Value::int(y), Value::int(m), Value::int(d)]))
    }

    fn _parse_iso_time(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let s = expect_str_arg(&args[0].value, "fromisoformat")?;
        let (h, m, se, us, tz) = parse_iso_time_bytes(s.as_bytes(), s)?;
        Ok(Value::tuple(vec![
            Value::int(h),
            Value::int(m),
            Value::int(se),
            Value::int(us),
            tz.map_or_else(Value::none, Value::int),
        ]))
    }

    fn _parse_iso_datetime(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let s = expect_str_arg(&args[0].value, "fromisoformat")?;
        let (y, mo, d, h, mi, se, us, tz) = parse_iso_datetime_bytes(s.as_bytes(), s)?;
        Ok(Value::tuple(vec![
            Value::int(y),
            Value::int(mo),
            Value::int(d),
            Value::int(h),
            Value::int(mi),
            Value::int(se),
            Value::int(us),
            tz.map_or_else(Value::none, Value::int),
        ]))
    }

    fn _format_strftime(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let fmt = expect_str_arg(&args[0].value, "strftime")?;
        let y = expect_int_arg(&args[1].value, "year")?;
        let mo = expect_int_arg(&args[2].value, "month")?;
        let d = expect_int_arg(&args[3].value, "day")?;
        let h = expect_int_arg(&args[4].value, "hour")?;
        let mi = expect_int_arg(&args[5].value, "minute")?;
        let se = expect_int_arg(&args[6].value, "second")?;
        let us = expect_int_arg(&args[7].value, "microsecond")?;
        let tz_us = match args[8].value.kind() {
            ValueKind::None => None,
            _ => Some(expect_int_arg(&args[8].value, "tz_offset")?),
        };
        let tz_name = match args[9].value.kind() {
            ValueKind::None => None,
            ValueKind::Str(s) => Some(s),
            _ => None,
        };
        Ok(Value::string(strftime_format(
            fmt, y, mo, d, h, mi, se, us, tz_us, tz_name,
        )))
    }

    fn _parse_strptime(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let text = expect_str_arg(&args[0].value, "strptime")?;
        let fmt = expect_str_arg(&args[1].value, "strptime")?;
        let (y, mo, d, h, mi, se, us, tz_us, tz_name) = strptime_parse(text, fmt)?;
        Ok(Value::tuple(vec![
            Value::int(y),
            Value::int(mo),
            Value::int(d),
            Value::int(h),
            Value::int(mi),
            Value::int(se),
            Value::int(us),
            tz_us.map_or_else(Value::none, Value::int),
            tz_name.map_or_else(Value::none, Value::string),
        ]))
    }
}
