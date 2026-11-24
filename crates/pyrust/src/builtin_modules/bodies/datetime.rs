// `datetime` module — body for the `datetime @inject` entry in
// `pyrust_builtin_modules!` (issue #2805).
//
// This file is the thin Python-registration and injection facade. Core
// Gregorian calendar math, ISO-8601 parsing, `strftime` formatting, and
// `strptime` parsing live in submodules under `bodies/datetime/`.
//
// Reference: <https://docs.python.org/3/library/datetime.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

#[path = "datetime/calendar.rs"]
mod calendar;
#[path = "datetime/iso_parse.rs"]
mod iso_parse;
#[path = "datetime/strftime.rs"]
mod strftime;
#[path = "datetime/strptime.rs"]
mod strptime;

use self::calendar::{
    MAX_ORDINAL, calc_iso_calendar, from_iso_cal, ord_to_ymd, validate_ymd, ymd_to_ord,
};
use self::iso_parse::{parse_iso_date_bytes, parse_iso_datetime_bytes, parse_iso_time_bytes};
use self::strftime::strftime_format;
use self::strptime::strptime_parse;

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
        .ok_or_else(|| PyError::runtime("datetime: exec namespace not a dict".into()))?;
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
