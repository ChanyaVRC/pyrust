//! `strftime` formatting implementation for `datetime`.

use super::calendar::{
    FULL_MONTHS, FULL_WEEKDAYS, SHORT_MONTHS, SHORT_WEEKDAYS, calc_iso_calendar, day_of_year,
    weekday_from_ord, ymd_to_ord,
};

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
pub(super) fn strftime_format(
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
