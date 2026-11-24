//! Proleptic Gregorian calendar arithmetic and locale-independent name tables
//! for `datetime`.

use crate::error::{PyError, Result};

pub(super) const MIN_YEAR: i64 = 1;
pub(super) const MAX_YEAR: i64 = 9999;
pub(super) const MAX_ORDINAL: i64 = 3_652_059;
const UNIX_EPOCH_ORDINAL: i64 = 719_163;

pub(super) const SHORT_WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
pub(super) const FULL_WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
pub(super) const SHORT_MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
pub(super) const FULL_MONTHS: [&str; 12] = [
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

pub(super) fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

pub(super) fn month_length(year: i64, month: i64) -> i64 {
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

pub(super) fn validate_ymd(year: i64, month: i64, day: i64) -> Result<()> {
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
pub(super) fn ymd_to_ord(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m_adj = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m_adj + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468 + UNIX_EPOCH_ORDINAL
}

/// Inverse of [`ymd_to_ord`] for `1 <= ord <= 3_652_059`.
pub(super) fn ord_to_ymd(ord: i64) -> (i64, i64, i64) {
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

pub(super) fn day_of_year(year: i64, month: i64, day: i64) -> i64 {
    ymd_to_ord(year, month, day) - ymd_to_ord(year, 1, 1) + 1
}

/// Monday = 0 .. Sunday = 6.
pub(super) fn weekday_from_ord(ord: i64) -> i64 {
    (ord - 1).rem_euclid(7)
}

/// Ordinal of Monday of ISO week 1 for `year`.
fn iso_week1_monday(year: i64) -> i64 {
    let jan4 = ymd_to_ord(year, 1, 4);
    jan4 - weekday_from_ord(jan4)
}

pub(super) fn calc_iso_calendar(year: i64, month: i64, day: i64) -> (i64, i64, i64) {
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

pub(super) fn from_iso_cal(iso_year: i64, iso_week: i64, iso_day: i64) -> Result<(i64, i64, i64)> {
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
