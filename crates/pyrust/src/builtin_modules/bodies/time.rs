// `time` module — wall-clock, monotonic and CPU clocks plus the
// `struct_time` calendar conversions (issue #2787).
//
// Included into `pub mod time { … }` declared by the
// `pyrust_builtin_modules!` invocation in `builtin_modules/mod.rs`.
//
// The clock / sleep functions are pure Rust (`std::time` + a process-wide
// monotonic epoch).  Calendar conversion (`gmtime` / `localtime` / `mktime` /
// `strftime`) and the timezone constants use the libc time routines, matching
// CPython's `Modules/timemodule.c`, which is also a thin libc wrapper.
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
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::error::{PyError, Result};
use crate::interpreter::{reject_keyword_args_expanded, ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};
use num_traits::ToPrimitive;
use pyrust_derive::pyrust_module;

/// Python-level members of the module (`struct_time`), defined as ordinary
/// Python source.
const TIME_PY_SOURCE: &str = include_str!("time_py.py");

/// Names from `TIME_PY_SOURCE` exported onto the `time` module.
const TIME_PY_EXPORTS: [&str; 1] = ["struct_time"];

thread_local! {
    /// Process-wide monotonic reference point.  `monotonic()` / `perf_counter()`
    /// report seconds elapsed from this instant, so the value is non-negative
    /// and never decreases for the process lifetime (CPython's `monotonic` is
    /// likewise anchored to an unspecified epoch).
    static MONO_EPOCH: Instant = Instant::now();
}

/// Execute `TIME_PY_SOURCE` once and copy its public names onto the `time`
/// module's attribute map.  Wired from `env.rs::load_module`'s post-import hook
/// (mirrors the `string` / `asyncio` injection).
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<()> {
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
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(())
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
    Err(PyError::Runtime("time: collections.namedtuple not available".into()))
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
        let mut tm = struct_time_to_tm(_interp, &args[0].value)?;
        // SAFETY: `mktime` reads/normalises the supplied `tm` in place; it
        // takes a valid pointer and has no other preconditions.
        let secs = unsafe { libc::mktime(&mut tm) };
        if secs == -1 {
            return Err(PyError::named(
                "OverflowError",
                "mktime argument out of range".to_string(),
            ));
        }
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
            struct_time_to_tm(_interp, &args[1].value)?
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
    MONO_EPOCH.with(|start| start.elapsed().as_secs_f64())
}

/// Monotonic nanoseconds elapsed since the process-wide epoch.
fn monotonic_nanos() -> u128 {
    MONO_EPOCH.with(|start| start.elapsed().as_nanos())
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
                "must be real number, not {}",
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
            format!(
                "{fn_name}() takes no arguments ({} given)",
                args.len()
            ),
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
        Some(_) => arg_to_f64(&args[0].value),
    }
}

// ── calendar conversion ──────────────────────────────────────────────────────

/// Convert `secs` (seconds since the epoch) into a broken-down libc `tm`,
/// either UTC (`local == false`, via `gmtime_r`) or local time
/// (`local == true`, via `localtime_r`).
#[cfg(unix)]
fn broken_down(secs: f64, local: bool) -> Result<libc::tm> {
    let t: libc::time_t = secs as libc::time_t;
    // SAFETY: both routines take a pointer to a `time_t` input and a pointer to
    // a caller-allocated `tm` output; we provide valid pointers to stack values
    // and check the (nullable) return before reading the result.
    let mut out: libc::tm = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        if local {
            libc::localtime_r(&t, &mut out)
        } else {
            libc::gmtime_r(&t, &mut out)
        }
    };
    if ret.is_null() {
        return Err(PyError::named(
            "OverflowError",
            "timestamp out of range for platform time_t".to_string(),
        ));
    }
    Ok(out)
}

#[cfg(not(unix))]
fn broken_down(_secs: f64, _local: bool) -> Result<libc::tm> {
    Err(PyError::named(
        "OSError",
        "time conversion is not supported on this platform".to_string(),
    ))
}

/// Build a `time.struct_time` instance from a broken-down libc `tm` by fetching
/// the (Python-defined) `struct_time` class off the imported module and calling
/// it with the nine fields, normalised to CPython's conventions:
///   * `tm_year` is the full year (libc stores year-1900),
///   * `tm_mon` is 1-based (libc is 0-based),
///   * `tm_wday` is 0=Monday (libc is 0=Sunday),
///   * `tm_yday` is 1-based (libc is 0-based).
fn make_struct_time(interp: &mut Interpreter, tm: &libc::tm) -> Result<Value> {
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

/// CPython-normalised nine `struct_time` fields from a libc `tm`.
fn tm_to_fields(tm: &libc::tm) -> [i64; 9] {
    [
        tm.tm_year as i64 + 1900,
        tm.tm_mon as i64 + 1,
        tm.tm_mday as i64,
        tm.tm_hour as i64,
        tm.tm_min as i64,
        tm.tm_sec as i64,
        // libc: 0=Sunday..6=Saturday → CPython: 0=Monday..6=Sunday.
        (tm.tm_wday as i64 + 6) % 7,
        tm.tm_yday as i64 + 1,
        tm.tm_isdst as i64,
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

/// Convert a 9-element `struct_time` (or any 9-element sequence, which CPython
/// also accepts) into a libc `tm` for `mktime` / `strftime`.
#[cfg(unix)]
fn struct_time_to_tm(interp: &mut Interpreter, val: &Value) -> Result<libc::tm> {
    let items = interp.collect_iterable(val)?;
    if items.len() != 9 {
        return Err(PyError::named(
            "TypeError",
            "argument must be a sequence of length 9".to_string(),
        ));
    }
    let f: Vec<i64> = items
        .iter()
        .map(field_to_i64)
        .collect::<Result<Vec<_>>>()?;
    // SAFETY: zero-initialise then fill every field libc reads; `tm_gmtoff` and
    // `tm_zone` stay zero/NULL, which the conversions tolerate.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = (f[0] - 1900) as libc::c_int;
    tm.tm_mon = (f[1] - 1) as libc::c_int;
    tm.tm_mday = f[2] as libc::c_int;
    tm.tm_hour = f[3] as libc::c_int;
    tm.tm_min = f[4] as libc::c_int;
    tm.tm_sec = f[5] as libc::c_int;
    // CPython: 0=Monday..6=Sunday → libc: 0=Sunday..6=Saturday.
    tm.tm_wday = ((f[6] + 1) % 7) as libc::c_int;
    tm.tm_yday = (f[7] - 1) as libc::c_int;
    tm.tm_isdst = f[8] as libc::c_int;
    Ok(tm)
}

#[cfg(not(unix))]
fn struct_time_to_tm(_interp: &mut Interpreter, _val: &Value) -> Result<libc::tm> {
    Err(PyError::named(
        "OSError",
        "time conversion is not supported on this platform".to_string(),
    ))
}

/// Coerce one `struct_time` field to `i64`, accepting int / bool.
fn field_to_i64(v: &Value) -> Result<i64> {
    match v.kind() {
        ValueKind::Int(i) => Ok(i),
        ValueKind::Bool(b) => Ok(b as i64),
        ValueKind::BigInt(b) => b.to_i64().ok_or_else(|| {
            PyError::named("OverflowError", "Python int too large to convert".to_string())
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

/// Format a libc `tm` with `fmt` via the platform `strftime(3)`.
#[cfg(unix)]
fn strftime_impl(fmt: &str, tm: &libc::tm) -> Result<String> {
    use std::ffi::CString;
    let cfmt = CString::new(fmt).map_err(|_| {
        PyError::named(
            "ValueError",
            "embedded null character in format string".to_string(),
        )
    })?;
    // strftime needs a generous buffer; grow until the result fits.
    let mut cap = 256usize.max(fmt.len() * 8 + 64);
    loop {
        let mut buf = vec![0u8; cap];
        // SAFETY: `strftime` writes at most `cap` bytes into `buf` (including the
        // NUL terminator) and reads the NUL-terminated `cfmt` plus `tm`; all
        // pointers are valid and `tm` is fully initialised by the caller.
        let n = unsafe {
            libc::strftime(
                buf.as_mut_ptr() as *mut libc::c_char,
                cap,
                cfmt.as_ptr(),
                tm,
            )
        };
        if n > 0 {
            buf.truncate(n);
            return Ok(String::from_utf8_lossy(&buf).into_owned());
        }
        // n == 0 is ambiguous (either overflow or a legitimately empty result).
        // Grow once up to a sane ceiling; if a larger buffer still yields 0 the
        // format genuinely produced the empty string.
        if cap >= 1 << 16 {
            return Ok(String::new());
        }
        cap *= 4;
    }
}

#[cfg(not(unix))]
fn strftime_impl(_fmt: &str, _tm: &libc::tm) -> Result<String> {
    Err(PyError::named(
        "OSError",
        "strftime is not supported on this platform".to_string(),
    ))
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

    // Current year, to sample a winter and a summer instant within it.
    let now = unix_time_secs();
    let year = broken_down(now, false)
        .map(|tm| tm.tm_year as i64 + 1900)
        .unwrap_or(2024);

    // Approximate Jan 15 and Jul 15 of `year` as UTC instants (good enough to
    // land on standard vs DST in both hemispheres).
    let days_from_epoch = |y: i64, ordinal: i64| -> i64 {
        // Days from 1970-01-01 to Jan 1 of year `y`, plus `ordinal` days.
        let mut days = 0i64;
        if y >= 1970 {
            for yy in 1970..y {
                days += if is_leap(yy) { 366 } else { 365 };
            }
        } else {
            for yy in y..1970 {
                days -= if is_leap(yy) { 366 } else { 365 };
            }
        }
        days + ordinal
    };
    let jan = (days_from_epoch(year, 14) * 86400) as f64;
    let jul = (days_from_epoch(year, 195) * 86400) as f64;

    let off = |secs: f64| -> Option<(i64, i32, String)> {
        broken_down(secs, true).ok().map(|tm| {
            let name = if tm.tm_zone.is_null() {
                String::new()
            } else {
                // SAFETY: `tm_zone` is a NUL-terminated string owned by libc's
                // timezone database, valid for the process lifetime.
                unsafe { CStr::from_ptr(tm.tm_zone) }
                    .to_string_lossy()
                    .into_owned()
            };
            (tm.tm_gmtoff as i64, tm.tm_isdst, name)
        })
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

/// Proleptic Gregorian leap-year test.
fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
