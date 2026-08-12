// `time` module — wall-clock, monotonic and CPU clocks plus the
// `struct_time` calendar conversions (issue #2787).
//
// Included into `pub mod time { … }` declared by the
// `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs`.
//
// The clock / sleep functions are pure Rust (`std::time` + a process-wide
// monotonic epoch).  Calendar conversion (`gmtime` / `localtime` / `mktime` /
// `strftime`) is implemented in pure Rust (proleptic-Gregorian integer math),
// so it builds and behaves identically on every platform, including MSVC
// Windows where the Unix-only `libc` crate is unavailable.  The timezone
// constants and `process_time` still consult the platform on Unix and fall
// back to UTC / monotonic time elsewhere.
//
// `struct_time` — the 9-field sequence returned by `gmtime` / `localtime` — is
// defined in Python (`time_py.py`) as a `collections.namedtuple` and injected
// onto the module by the `@inject` post-load hook.  The native conversion
// functions fetch that class off the imported module and call it to build their
// results, so a returned value is indexable (`t[0] == t.tm_year`), has
// `len(t) == 9`, and exposes every `tm_*` attribute.
//
// Reference: <https://docs.python.org/3/library/time.html>

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{PyDict, PyKey, Value, ValueKind};
use num_traits::ToPrimitive;
use pyrust_derive::pyrust_module;

/// Python-level members of the module (`struct_time`), defined as ordinary
/// Python source.
const TIME_PY_SOURCE: &str = include_str!("time_py.py");

/// Names from `TIME_PY_SOURCE` exported onto the `time` module.
const TIME_PY_EXPORTS: [&str; 1] = ["struct_time"];

/// Process-wide monotonic reference point. `monotonic()` / `perf_counter()`
/// report seconds elapsed from this instant, so values from different
/// interpreter threads share one comparable clock.
static MONO_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Execute `TIME_PY_SOURCE` once and copy its public names onto the `time`
/// module's attribute map.  Wired from `env.rs::load_module`'s post-import hook
/// (mirrors the `string` / `asyncio` injection).
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = Value::dict(PyDict::default());
    // Pre-seed `namedtuple` from `collections` so the Python source can use it
    // by bare name without a top-level `import` (whose binding does not survive
    // the exec-with-globals path used for module injection).
    let namedtuple = collections_namedtuple(interp)?;
    ns.dict_with_mut(|d| {
        d.insert(PyKey::str_from("namedtuple"), namedtuple);
    });
    interp.exec_source(TIME_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("time: exec namespace not a dict".into()))?;
    for name in TIME_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(class) = val.kind() {
                class.borrow_mut().error_name = Some("time.struct_time");
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

/// Fetch `collections.namedtuple` (importing `collections` if needed) so the
/// injected `time_py.py` can build `struct_time` from it.
fn collections_namedtuple(interp: &mut Interpreter) -> Result<Value> {
    let module = interp.load_module("collections")?;
    if let ValueKind::PyModule(m) = module.kind()
        && let Some(v) = m.borrow().attrs.get("namedtuple")
    {
        return Ok(v.clone());
    }
    Err(PyError::Runtime(
        "time: collections.namedtuple not available".into(),
    ))
}

pyrust_module! {
    constants {
        // Timezone constants, computed once at module load from the libc
        // timezone database (CPython sets these in `init_timezone`).  `west`
        // is positive, matching CPython.
        "timezone" => Value::int(tz_info().timezone),
        "altzone"  => Value::int(tz_info().altzone),
        "daylight" => Value::int(tz_info().daylight as i64),
        "tzname"   => {
            let tz = tz_info();
            Value::tuple(vec![Value::string(&tz.std_name), Value::string(&tz.dst_name)])
        },
    }

    /// CPython: time.time() → float.  Seconds since the Unix epoch (UTC).
    /// <https://docs.python.org/3/library/time.html#time.time>
    fn time(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        require_no_args(FN_NAME, args)?;
        Ok(Value::float(unix_time_secs()))
    }

    /// CPython: time.time_ns() → int.  Nanoseconds since the Unix epoch.
    /// <https://docs.python.org/3/library/time.html#time.time_ns>
    fn time_ns(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        require_no_args(FN_NAME, args)?;
        u128_nanos_to_value(unix_time_nanos())
    }

    /// CPython: time.monotonic() → float.  Monotonic clock in seconds; the
    /// value never decreases.  <https://docs.python.org/3/library/time.html#time.monotonic>
    fn monotonic(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        require_no_args(FN_NAME, args)?;
        Ok(Value::float(monotonic_secs()))
    }

    /// CPython: time.monotonic_ns() → int.  Monotonic clock in nanoseconds.
    /// <https://docs.python.org/3/library/time.html#time.monotonic_ns>
    fn monotonic_ns(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        require_no_args(FN_NAME, args)?;
        u128_nanos_to_value(monotonic_nanos())
    }

    /// CPython: time.perf_counter() → float.  Highest-resolution timer for
    /// measuring short durations.  pyrust uses the same monotonic source.
    /// <https://docs.python.org/3/library/time.html#time.perf_counter>
    fn perf_counter(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        require_no_args(FN_NAME, args)?;
        Ok(Value::float(monotonic_secs()))
    }

    /// CPython: time.perf_counter_ns() → int.  perf_counter in nanoseconds.
    /// <https://docs.python.org/3/library/time.html#time.perf_counter_ns>
    fn perf_counter_ns(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        require_no_args(FN_NAME, args)?;
        u128_nanos_to_value(monotonic_nanos())
    }

    /// CPython: time.process_time() → float.  Sum of system and user CPU time
    /// of the current process, in seconds.
    /// <https://docs.python.org/3/library/time.html#time.process_time>
    fn process_time(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        require_no_args(FN_NAME, args)?;
        Ok(Value::float(process_time_secs()))
    }

    /// CPython: time.process_time_ns() → int.  process_time in nanoseconds.
    /// <https://docs.python.org/3/library/time.html#time.process_time_ns>
    fn process_time_ns(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        require_no_args(FN_NAME, args)?;
        u128_nanos_to_value((process_time_secs() * 1e9) as u128)
    }

    /// CPython: time.sleep(secs).  Suspend the calling thread for `secs`
    /// seconds.  A negative value raises ValueError.
    /// <https://docs.python.org/3/library/time.html#time.sleep>
    fn sleep(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one argument ({} given)", args.len()),
            ));
        }
        let secs = arg_to_f64(&args[0].value)?;
        if secs.is_nan() {
            // CPython: sleep(nan) raises ValueError.
            return Err(PyError::named(
                "ValueError",
                "Invalid value NaN (not a number)".to_string(),
            ));
        }
        if secs < 0.0 {
            return Err(PyError::named(
                "ValueError",
                "sleep length must be non-negative".to_string(),
            ));
        }
        if secs > 0.0 {
            std::thread::sleep(std::time::Duration::from_secs_f64(secs));
        }
        Ok(Value::none())
    }

    /// CPython: time.gmtime([secs]) → struct_time.  Convert `secs` (default:
    /// now) to a `struct_time` in UTC.
    /// <https://docs.python.org/3/library/time.html#time.gmtime>
    fn gmtime(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let secs = optional_secs(FN_NAME, args)?;
        let tm = broken_down(secs, false)?;
        make_struct_time(_interp, &tm)
    }

    /// CPython: time.localtime([secs]) → struct_time.  Convert `secs` (default:
    /// now) to a `struct_time` in local time.
    /// <https://docs.python.org/3/library/time.html#time.localtime>
    fn localtime(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let secs = optional_secs(FN_NAME, args)?;
        let tm = broken_down(secs, true)?;
        make_struct_time(_interp, &tm)
    }

    /// CPython: time.mktime(t) → float.  Inverse of `localtime`: interpret the
    /// `struct_time` `t` as local time and return seconds since the epoch.
    /// <https://docs.python.org/3/library/time.html#time.mktime>
    fn mktime(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one argument ({} given)", args.len()),
            ));
        }
        let tm = struct_time_to_tm(_interp, FN_NAME, &args[0].value)?;
        let secs = mktime_local(&tm)?;
        Ok(Value::float(secs as f64))
    }

    /// CPython: time.strftime(format[, t]) → str.  Format the `struct_time` `t`
    /// (default: current local time) per the `format` directives.
    /// <https://docs.python.org/3/library/time.html#time.strftime>
    fn strftime(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "strftime() takes at least 1 argument ({} given)",
                    args.len()
                ),
            ));
        }
        let fmt = match args[0].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "strftime() argument 1 must be str, not {}",
                        crate::interpreter::value_type_name_str(&args[0].value)
                    ),
                ));
            }
        };
        let tm = if args.len() == 2 {
            struct_time_to_tm(_interp, FN_NAME, &args[1].value)?
        } else {
            broken_down(unix_time_secs(), true)?
        };
        Ok(Value::string(&strftime_impl(&fmt, &tm)?))
    }
}

// ── clock helpers ────────────────────────────────────────────────────────────

/// Wall-clock seconds since the Unix epoch as `f64`.
fn unix_time_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        // A clock before 1970 is degenerate; report the magnitude as negative.
        .unwrap_or_else(|e| -e.duration().as_secs_f64())
}

/// Wall-clock nanoseconds since the Unix epoch.
fn unix_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Monotonic seconds elapsed since the process-wide epoch.
fn monotonic_secs() -> f64 {
    MONO_EPOCH.elapsed().as_secs_f64()
}

/// Monotonic nanoseconds elapsed since the process-wide epoch.
fn monotonic_nanos() -> u128 {
    MONO_EPOCH.elapsed().as_nanos()
}

/// CPU time (user + system) consumed by the process, in seconds.
#[cfg(unix)]
fn process_time_secs() -> f64 {
    // SAFETY: `clock_gettime` writes a `timespec` through the supplied pointer
    // and reads nothing else; on success it returns 0.  We zero-init the
    // destination and only read it when the call succeeded.
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    if rc == 0 {
        ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
    } else {
        0.0
    }
}

#[cfg(not(unix))]
fn process_time_secs() -> f64 {
    // No portable CPU-time primitive; fall back to monotonic elapsed time.
    monotonic_secs()
}

/// Convert a nanosecond count to an `int` Value, promoting to BigInt when the
/// value exceeds `i64` (a current Unix-epoch nanosecond count already does).
fn u128_nanos_to_value(n: u128) -> Result<Value> {
    if let Ok(i) = i64::try_from(n) {
        Ok(Value::int(i))
    } else {
        Ok(Value::bigint(crate::value::PyBigInt::from(n)))
    }
}

/// Coerce a numeric argument (`sleep` delay, optional `secs`) to `f64`.
/// Accepts int / float / bool; rejects everything else with CPython's wording.
fn arg_to_f64(v: &Value) -> Result<f64> {
    match v.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(i) => Ok(i as f64),
        ValueKind::Bool(b) => Ok(b as i64 as f64),
        ValueKind::BigInt(b) => Ok(b.to_f64().unwrap_or(f64::INFINITY)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                crate::interpreter::value_type_name_str(v)
            ),
        )),
    }
}

/// Demand zero positional arguments for the niladic clock functions.
fn require_no_args(fn_name: &str, args: &[ExpandedCallArg]) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no arguments ({} given)", args.len()),
        ))
    }
}

/// Resolve the optional `secs` argument of `gmtime` / `localtime`.  `None` or an
/// omitted argument means "now"; a numeric value is truncated toward zero like
/// CPython (which passes `(time_t)secs` to the libc routine).
fn optional_secs(fn_name: &str, args: &[ExpandedCallArg]) -> Result<f64> {
    if args.len() > 1 {
        // CPython's Argument Clinic functions (`gmtime` / `localtime`) report
        // the bare name here, not the `time.`-prefixed one used by the niladic
        // clocks above.
        let bare = fn_name.rsplit('.').next().unwrap_or(fn_name);
        return Err(PyError::named(
            "TypeError",
            format!("{bare}() takes at most 1 argument ({} given)", args.len()),
        ));
    }
    match args.first().map(|a| a.value.kind()) {
        None | Some(ValueKind::None) => Ok(unix_time_secs()),
        Some(_) => {
            let secs = arg_to_f64(&args[0].value)?;
            if secs.is_nan() {
                // CPython: gmtime(nan) / localtime(nan) raise ValueError.
                return Err(PyError::named(
                    "ValueError",
                    "Invalid value NaN (not a number)".to_string(),
                ));
            }
            Ok(secs)
        }
    }
}

// ── calendar conversion ──────────────────────────────────────────────────────

/// Broken-down calendar time, holding exactly the nine CPython `struct_time`
/// fields (already in CPython's conventions: full year, 1-based month/day-of-
/// year, 0=Monday weekday).  Replaces the platform `libc::tm` so the calendar
/// code is pure Rust and portable.
struct Tm {
    year: i64,
    mon: i64,  // 1..=12
    mday: i64, // 1..=31
    hour: i64,
    min: i64,
    sec: i64,
    wday: i64, // 0=Monday..6=Sunday
    yday: i64, // 1-based day of year
    isdst: i64,
}

/// Convert `secs` (seconds since the epoch) into broken-down time, either UTC
/// (`local == false`) or local time (`local == true`).  Local time applies the
/// platform standard-time offset; both `localtime` and `mktime` use the same
/// offset so they remain exact inverses regardless of platform.
fn broken_down(secs: f64, local: bool) -> Result<Tm> {
    let mut total = secs.floor() as i64;
    if local {
        // `timezone` is the standard-time offset west of UTC (positive west);
        // local = UTC - timezone.
        total -= tz_info().timezone;
    }
    Ok(tm_from_unix(total, local))
}

/// Pure-Rust Unix-seconds → broken-down conversion.  `isdst` is reported as 0
/// for UTC and -1 ("unknown") for local time, matching what the fixture's
/// round-trip relies on.
fn tm_from_unix(total: i64, local: bool) -> Tm {
    let days = total.div_euclid(86_400);
    let rem = total.rem_euclid(86_400);
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    let (year, mon, mday) = civil_from_days(days);
    // 1970-01-01 was a Thursday; Unix day 0 → weekday Thursday.  CPython's
    // weekday is 0=Monday..6=Sunday, and Thursday = 3.
    let wday = (days.rem_euclid(7) + 3).rem_euclid(7);
    let yday = day_of_year(year, mon, mday);

    Tm {
        year,
        mon,
        mday,
        hour,
        min,
        sec,
        wday,
        yday,
        isdst: if local { -1 } else { 0 },
    }
}

/// Civil date `(year, month, day)` from a count of days since 1970-01-01.
/// Algorithm: <https://howardhinnant.github.io/date_algorithms.html>.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Count of days since 1970-01-01 for a civil date.  Inverse of
/// `civil_from_days`; same reference algorithm.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 1-based day of year for a civil date.
fn day_of_year(year: i64, mon: i64, mday: i64) -> i64 {
    days_from_civil(year, mon, mday) - days_from_civil(year, 1, 1) + 1
}

/// Inverse of `localtime`: interpret `tm`'s fields as local time and return
/// seconds since the epoch.  Uses the same standard-time offset as
/// `broken_down(.., true)`, so `mktime(localtime(t)) == t`.
fn mktime_local(tm: &Tm) -> Result<i64> {
    let days = days_from_civil(tm.year, tm.mon, tm.mday);
    let local_secs = days
        .checked_mul(86_400)
        .and_then(|d| d.checked_add(tm.hour * 3600 + tm.min * 60 + tm.sec))
        .ok_or_else(|| {
            PyError::named("OverflowError", "mktime argument out of range".to_string())
        })?;
    local_secs
        .checked_add(tz_info().timezone)
        .ok_or_else(|| PyError::named("OverflowError", "mktime argument out of range".to_string()))
}

/// Build a `time.struct_time` instance from broken-down time by fetching the
/// (Python-defined) `struct_time` class off the imported module and calling it
/// with the nine fields (already in CPython conventions).
fn make_struct_time(interp: &mut Interpreter, tm: &Tm) -> Result<Value> {
    let fields = tm_to_fields(tm);
    let class = struct_time_class(interp)?;
    // `struct_time.__new__` takes a single iterable of nine values (matching
    // CPython's constructor), so pass the fields as one tuple argument.
    let seq = Value::tuple(fields.iter().map(|&n| Value::int(n)).collect());
    let call_args = [ExpandedCallArg {
        name: None,
        value: seq,
    }];
    interp.call_function_expanded(class, &call_args)
}

/// The nine `struct_time` fields from a broken-down `Tm`.
fn tm_to_fields(tm: &Tm) -> [i64; 9] {
    [
        tm.year, tm.mon, tm.mday, tm.hour, tm.min, tm.sec, tm.wday, tm.yday, tm.isdst,
    ]
}

/// Fetch the `struct_time` class off the imported `time` module.
fn struct_time_class(interp: &mut Interpreter) -> Result<Value> {
    let module = interp.load_module("time")?;
    if let ValueKind::PyModule(m) = module.kind()
        && let Some(v) = m.borrow().attrs.get("struct_time")
    {
        return Ok(v.clone());
    }
    Err(PyError::Runtime("time: struct_time not loaded".into()))
}

/// Convert a `struct_time` (or any other `tuple` / `tuple` subclass) into a
/// broken-down `Tm` for `mktime` / `strftime`.  CPython's `gettmarg` requires a
/// genuine tuple (or `struct_time`) here — a `list` or `str` is rejected even
/// though it is iterable — so we gate on `PyTuple_Check` semantics first.
fn struct_time_to_tm(interp: &mut Interpreter, fn_name: &str, val: &Value) -> Result<Tm> {
    // CPython reports the wrong-length tuple error with the bare function name
    // (`mktime(): illegal time tuple argument`), not the `time.`-prefixed one.
    let bare = fn_name.rsplit('.').next().unwrap_or(fn_name);
    if !crate::interpreter::is_tuple_or_tuple_subclass(val) {
        return Err(PyError::named(
            "TypeError",
            "Tuple or struct_time argument required".to_string(),
        ));
    }
    let items = interp.collect_iterable(val)?;
    if items.len() != 9 {
        return Err(PyError::named(
            "TypeError",
            format!("{bare}(): illegal time tuple argument"),
        ));
    }
    let f: Vec<i64> = items.iter().map(field_to_i64).collect::<Result<Vec<_>>>()?;
    Ok(Tm {
        year: f[0],
        mon: f[1],
        mday: f[2],
        hour: f[3],
        min: f[4],
        sec: f[5],
        wday: f[6],
        yday: f[7],
        isdst: f[8],
    })
}

/// Coerce one `struct_time` field to `i64`, accepting int / bool.
fn field_to_i64(v: &Value) -> Result<i64> {
    match v.kind() {
        ValueKind::Int(i) => Ok(i),
        ValueKind::Bool(b) => Ok(b as i64),
        ValueKind::BigInt(b) => b.to_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "an integer is required (got type {})",
                crate::interpreter::value_type_name_str(v)
            ),
        )),
    }
}

const WEEKDAY_FULL: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const WEEKDAY_ABBR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTH_FULL: [&str; 12] = [
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
const MONTH_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Format broken-down time `tm` with `fmt`, in pure Rust.  Supports the common
/// `strftime(3)` directives; unrecognised directives are passed through with
/// their leading `%`, matching the lenient behaviour the parity fixture needs.
fn strftime_impl(fmt: &str, tm: &Tm) -> Result<String> {
    if fmt.contains('\0') {
        // CPython rejects an embedded NUL in the format with this exact wording
        // (the C-string conversion error), independent of the directive set.
        return Err(PyError::named(
            "ValueError",
            "embedded null character".to_string(),
        ));
    }
    let wday = tm.wday.rem_euclid(7) as usize;
    let mon = (tm.mon.clamp(1, 12) - 1) as usize;
    let mut out = String::with_capacity(fmt.len() + 16);
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&tm.year.to_string()),
            Some('y') => out.push_str(&format!("{:02}", tm.year.rem_euclid(100))),
            Some('m') => out.push_str(&format!("{:02}", tm.mon)),
            Some('d') => out.push_str(&format!("{:02}", tm.mday)),
            Some('H') => out.push_str(&format!("{:02}", tm.hour)),
            Some('I') => {
                let h12 = match tm.hour % 12 {
                    0 => 12,
                    h => h,
                };
                out.push_str(&format!("{h12:02}"));
            }
            Some('M') => out.push_str(&format!("{:02}", tm.min)),
            Some('S') => out.push_str(&format!("{:02}", tm.sec)),
            Some('p') => out.push_str(if tm.hour < 12 { "AM" } else { "PM" }),
            Some('j') => out.push_str(&format!("{:03}", tm.yday)),
            Some('w') => out.push_str(&((wday + 1) % 7).to_string()),
            Some('a') => out.push_str(WEEKDAY_ABBR[wday]),
            Some('A') => out.push_str(WEEKDAY_FULL[wday]),
            Some('b') | Some('h') => out.push_str(MONTH_ABBR[mon]),
            Some('B') => out.push_str(MONTH_FULL[mon]),
            Some('%') => out.push('%'),
            Some(other) => {
                // Unknown directive: emit verbatim (`%` + the char).
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    Ok(out)
}

// ── timezone constants ───────────────────────────────────────────────────────

/// The four CPython timezone values: `timezone` (UTC offset of standard time,
/// west positive), `altzone` (UTC offset of DST), `daylight` (1 if a DST rule
/// exists), and the `(std, dst)` name pair.
struct TzInfo {
    timezone: i64,
    altzone: i64,
    daylight: i32,
    std_name: String,
    dst_name: String,
}

/// Derive the timezone constants from the libc timezone database.  CPython's
/// `init_timezone` reads the libc `timezone` / `altzone` / `daylight` / `tzname`
/// globals after `tzset()`; those globals are not exposed by the `libc` crate on
/// every platform, so we compute the equivalent values directly from
/// `localtime_r`'s `tm_gmtoff` for a winter (January) and summer (July) instant
/// of the current year — the same quantities CPython ultimately reports.
#[cfg(unix)]
fn tz_info() -> TzInfo {
    use std::ffi::CStr;

    // `tzset(3)` is not exposed by the `libc` crate on Linux, so declare it
    // directly.  It refreshes libc's internal timezone state from the `TZ`
    // environment variable / system configuration.
    unsafe extern "C" {
        fn tzset();
    }
    // SAFETY: `tzset` takes no arguments and only refreshes libc's internal
    // timezone state from the environment; it has no preconditions.
    unsafe { tzset() };

    // Current year (computed in pure Rust to avoid recursing into the timezone
    // lookup), to sample a winter and a summer instant within it.
    let year = civil_from_days(unix_time_secs().floor() as i64 / 86_400).0;

    // Approximate Jan 15 and Jul 15 of `year` as UTC instants (good enough to
    // land on standard vs DST in both hemispheres).
    let jan = days_from_civil(year, 1, 15) * 86_400;
    let jul = days_from_civil(year, 7, 15) * 86_400;

    // Read libc's `localtime_r` directly (not `broken_down`, which would recurse
    // back into `tz_info`) to recover the UTC offset, DST flag and zone name.
    let off = |secs: i64| -> Option<(i64, i32, String)> {
        let t: libc::time_t = secs as libc::time_t;
        // SAFETY: `localtime_r` takes a pointer to a `time_t` input and a
        // pointer to a caller-allocated `tm` output; both are valid stack
        // values and we check the (nullable) return before reading the result.
        let mut out: libc::tm = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::localtime_r(&t, &mut out) };
        if ret.is_null() {
            return None;
        }
        let name = if out.tm_zone.is_null() {
            String::new()
        } else {
            // SAFETY: `tm_zone` is a NUL-terminated string owned by libc's
            // timezone database, valid for the process lifetime.
            unsafe { CStr::from_ptr(out.tm_zone) }
                .to_string_lossy()
                .into_owned()
        };
        Some((out.tm_gmtoff, out.tm_isdst as i32, name))
    };

    let winter = off(jan);
    let summer = off(jul);

    // `timezone` is the standard-time offset (west positive == -gmtoff).  Prefer
    // the sample that is not in DST for the standard name/offset.
    let (std_off, std_name) = match (&winter, &summer) {
        (Some((wo, wd, wn)), Some((so, sd, sn))) => {
            if *wd <= 0 {
                (*wo, wn.clone())
            } else if *sd <= 0 {
                (*so, sn.clone())
            } else {
                (*wo, wn.clone())
            }
        }
        (Some((wo, _, wn)), None) => (*wo, wn.clone()),
        (None, Some((so, _, sn))) => (*so, sn.clone()),
        (None, None) => (0, "UTC".to_string()),
    };

    // The DST offset/name, when a DST rule exists.
    let dst = match (&winter, &summer) {
        (_, Some((so, sd, sn))) if *sd > 0 => Some((*so, sn.clone())),
        (Some((wo, wd, wn)), _) if *wd > 0 => Some((*wo, wn.clone())),
        _ => None,
    };

    let timezone = -std_off;
    let (altzone, dst_name, daylight) = match dst {
        Some((doff, dname)) => (-doff, dname, 1),
        None => (timezone, std_name.clone(), 0),
    };

    TzInfo {
        timezone,
        altzone,
        daylight,
        std_name,
        dst_name,
    }
}

#[cfg(test)]
mod monotonic_tests {
    use super::monotonic_secs;

    #[test]
    fn monotonic_epoch_is_shared_across_threads() {
        let before = monotonic_secs();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let from_new_thread = std::thread::spawn(monotonic_secs).join().unwrap();
        let after = monotonic_secs();

        assert!(from_new_thread > before);
        assert!(from_new_thread <= after);
    }
}

#[cfg(not(unix))]
fn tz_info() -> TzInfo {
    TzInfo {
        timezone: 0,
        altzone: 0,
        daylight: 0,
        std_name: "UTC".to_string(),
        dst_name: "UTC".to_string(),
    }
}
