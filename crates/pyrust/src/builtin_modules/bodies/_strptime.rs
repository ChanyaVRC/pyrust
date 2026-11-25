// `_strptime` internal module — 100% native-Rust Python 3.12 strptime helper
// module delegating to `time.strptime` and `datetime.datetime.strptime`
// (issue #3250).
//
// Exposes `LocaleTime`, `TimeRE`, `_strptime`, `_strptime_time`,
// `_strptime_datetime`, and `IGNORECASE`.
//
// Reference: CPython 3.12 `Lib/_strptime.py`

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::builtin_args::{PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};

const DEFAULT_FMT: &str = "%a %b %d %H:%M:%S %Y";

const A_WEEKDAY: &[&str] = &["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const F_WEEKDAY: &[&str] = &[
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];
const A_MONTH: &[&str] = &[
    "", "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const F_MONTH: &[&str] = &[
    "",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

fn call_positional(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn str_list(items: &[&str]) -> Value {
    Value::list(items.iter().map(|s| Value::string(*s)).collect())
}

fn parse_via_time(interp: &mut Interpreter, data: &str, fmt: &str) -> Result<Value> {
    let dt = parse_via_datetime(interp, data, fmt)?;
    let tt_fn = interp.get_attr(&dt, "timetuple")?;
    call_positional(interp, tt_fn, &[])
}

fn parse_via_datetime(interp: &mut Interpreter, data: &str, fmt: &str) -> Result<Value> {
    let dt_mod = interp.load_module("datetime")?;
    let dt_cls = interp.get_attr(&dt_mod, "datetime")?;
    let strptime_fn = interp.get_attr(&dt_cls, "strptime")?;
    call_positional(
        interp,
        strptime_fn,
        &[Value::string(data), Value::string(fmt)],
    )
}

pyrust_module! {
    constants {
        "IGNORECASE" => Value::int(2),
    }

    class LocaleTime {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "lang",
                Value::tuple(vec![Value::none(), Value::none()]),
            )?;
            _interp.assign_attr(self_val.0.clone(), "a_weekday", str_list(A_WEEKDAY))?;
            _interp.assign_attr(self_val.0.clone(), "f_weekday", str_list(F_WEEKDAY))?;
            _interp.assign_attr(self_val.0.clone(), "a_month", str_list(A_MONTH))?;
            _interp.assign_attr(self_val.0.clone(), "f_month", str_list(F_MONTH))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "am_pm",
                Value::list(vec![Value::string("am"), Value::string("pm")]),
            )?;
            _interp.assign_attr(self_val.0.clone(), "LC_date_time", Value::string("%a %b %d %H:%M:%S %Y"))?;
            _interp.assign_attr(self_val.0.clone(), "LC_date", Value::string("%m/%d/%y"))?;
            _interp.assign_attr(self_val.0.clone(), "LC_time", Value::string("%H:%M:%S"))?;
            let tz_std = call_positional(
                _interp,
                Value::builtin_function("frozenset"),
                &[Value::list(vec![Value::string("utc"), Value::string("gmt")])],
            )?;
            let tz_dst = call_positional(
                _interp,
                Value::builtin_function("frozenset"),
                &[],
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "timezone",
                Value::tuple(vec![tz_std, tz_dst]),
            )?;
            Ok(Value::none())
        }
    }

    class TimeRE("dict") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(PyValue(Value::none()))] locale_time: PyValue,
        ) -> Result<Value> {
            let lt = if locale_time.0.is_none() {
                let m = _interp.load_module("_strptime")?;
                let lt_cls = _interp.get_attr(&m, "LocaleTime")?;
                call_positional(_interp, lt_cls, &[])?
            } else {
                locale_time.0.clone()
            };
            _interp.assign_attr(self_val.0.clone(), "locale_time", lt)?;
            let mut entries = PyDict::default();
            for &(k, v) in &[
                ("d", r"(?P<d>3[0-1]|[1-2]\d|0[1-9]|[1-9]| [1-9])"),
                ("f", r"(?P<f>[0-9]{1,6})"),
                ("H", r"(?P<H>2[0-3]|[0-1]\d|\d)"),
                ("I", r"(?P<I>1[0-2]|0[1-9]|[1-9])"),
                ("G", r"(?P<G>\d\d\d\d)"),
                ("j", r"(?P<j>36[0-6]|3[0-5]\d|[1-2]\d\d|0[1-9]\d|00[1-9]|[1-9]\d|0[1-9]|[1-9])"),
                ("m", r"(?P<m>1[0-2]|0[1-9]|[1-9])"),
                ("M", r"(?P<M>[0-5]\d|\d)"),
                ("S", r"(?P<S>6[0-1]|[0-5]\d|\d)"),
                ("U", r"(?P<U>5[0-3]|[0-4]\d|\d)"),
                ("w", r"(?P<w>[0-6])"),
                ("u", r"(?P<u>[1-7])"),
                ("V", r"(?P<V>5[0-3]|0[1-9]|[1-4]\d|\d)"),
                ("y", r"(?P<y>\d\d)"),
                ("Y", r"(?P<Y>\d\d\d\d)"),
                ("z", r"(?P<z>[+-]\d\d:?[0-5]\d(:?[0-5]\d(\.\d{1,6})?)?|(?-i:Z))"),
                ("%", "%"),
            ] {
                entries.insert(PyKey::str_from(k), Value::string(v));
            }
            let update_fn = _interp.get_attr(self_val.0, "update")?;
            call_positional(_interp, update_fn, &[Value::dict(entries)])?;
            Ok(Value::none())
        }

        fn pattern(#[positional_only] self_val: PyValueRef<'_>, format: PyStr<'_>) -> Result<Value> {
            let fmt: &str = &format;
            let mut out = String::new();
            let mut chars = fmt.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '%'
                    && let Some(spec) = chars.next()
                {
                    let key = spec.to_string();
                    let get_fn = _interp.get_attr(self_val.0, "get")?;
                    let repl = call_positional(_interp, get_fn, &[Value::string(&key)])?;
                    if let Some(rs) = repl.as_str() {
                        out.push_str(rs);
                    } else {
                        out.push('%');
                        out.push(spec);
                    }
                } else {
                    out.push(c);
                }
            }
            Ok(Value::string(out))
        }

        fn compile(#[positional_only] self_val: PyValueRef<'_>, format: PyStr<'_>) -> Result<Value> {
            let pat_fn = _interp.get_attr(self_val.0, "pattern")?;
            let pat = call_positional(_interp, pat_fn, &[Value::string(&*format)])?;
            let re_mod = _interp.load_module("re")?;
            let compile_fn = _interp.get_attr(&re_mod, "compile")?;
            call_positional(_interp, compile_fn, &[pat, Value::int(2)])
        }
    }

    fn _strptime(
        data_string: PyStr<'_>,
        #[default(DEFAULT_FMT.into())] format: PyStr<'_>,
    ) -> Result<Value> {
        let dt = parse_via_datetime(_interp, &data_string, &format)?;
        let tt_fn = _interp.get_attr(&dt, "timetuple")?;
        let tt = call_positional(_interp, tt_fn, &[])?;
        let items = _interp.collect_iterable(&tt)?;
        let microsecond = _interp.get_attr(&dt, "microsecond")?;
        let utcoffset_fn = _interp.get_attr(&dt, "utcoffset")?;
        let offset = call_positional(_interp, utcoffset_fn, &[])?;
        let gmtoff = if offset.is_none() {
            Value::none()
        } else {
            let total_sec_fn = _interp.get_attr(&offset, "total_seconds")?;
            let secs = call_positional(_interp, total_sec_fn, &[])?;
            if let ValueKind::Float(f) = secs.kind() {
                Value::int(f as i64)
            } else {
                secs
            }
        };
        let tzname_fn = _interp.get_attr(&dt, "tzname")?;
        let tzname = call_positional(_interp, tzname_fn, &[])?;
        let mut tuple_elems = items;
        tuple_elems.push(tzname);
        tuple_elems.push(gmtoff);
        Ok(Value::tuple(vec![
            Value::tuple(tuple_elems),
            microsecond,
            Value::none(),
        ]))
    }

    fn _strptime_time(
        data_string: PyStr<'_>,
        #[default(DEFAULT_FMT.into())] format: PyStr<'_>,
    ) -> Result<Value> {
        parse_via_time(_interp, &data_string, &format)
    }

    fn _strptime_datetime(
        cls: PyValueRef<'_>,
        data_string: PyStr<'_>,
        #[default(DEFAULT_FMT.into())] format: PyStr<'_>,
    ) -> Result<Value> {
        let dt = parse_via_datetime(_interp, &data_string, &format)?;
        let year = _interp.get_attr(&dt, "year")?;
        let month = _interp.get_attr(&dt, "month")?;
        let day = _interp.get_attr(&dt, "day")?;
        let hour = _interp.get_attr(&dt, "hour")?;
        let minute = _interp.get_attr(&dt, "minute")?;
        let second = _interp.get_attr(&dt, "second")?;
        let microsecond = _interp.get_attr(&dt, "microsecond")?;
        let tzinfo = _interp.get_attr(&dt, "tzinfo")?;
        call_positional(
            _interp,
            cls.0.clone(),
            &[year, month, day, hour, minute, second, microsecond, tzinfo],
        )
    }
}
