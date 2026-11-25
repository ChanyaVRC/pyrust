// `logging.handlers` submodule — 100% native Rust implementation of Python 3.12's
// `logging.handlers` module (issue #3251).
//
// Implements `BaseRotatingHandler`, `RotatingFileHandler`,
// `TimedRotatingFileHandler`, `WatchedFileHandler`, `BufferingHandler`,
// `MemoryHandler`, `QueueHandler`, `QueueListener`, `SocketHandler`,
// `DatagramHandler`, `SysLogHandler`, `HTTPHandler`, and `SMTPHandler`.
//
// Reference: <https://docs.python.org/3/library/logging.handlers.html>

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyIndex, PyPath, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, values_are_identical};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

fn value_as_f64(v: &Value) -> Option<f64> {
    match v.kind() {
        ValueKind::Float(f) => Some(f),
        ValueKind::Int(n) => Some(n as f64),
        ValueKind::Bool(b) => Some(if b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

pub(crate) const DEFAULT_TCP_LOGGING_PORT: i64 = 9020;
pub(crate) const DEFAULT_UDP_LOGGING_PORT: i64 = 9021;
pub(crate) const DEFAULT_HTTP_LOGGING_PORT: i64 = 9022;
pub(crate) const DEFAULT_SOAP_LOGGING_PORT: i64 = 9023;
pub(crate) const SYSLOG_UDP_PORT: i64 = 514;
pub(crate) const SYSLOG_TCP_PORT: i64 = 514;

const _MIDNIGHT: i64 = 24 * 60 * 60;

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_pos(interp, f, args)
}

fn logging_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("logging")?;
    interp.get_attr(&m, name)
}

fn call_class_method(
    interp: &mut Interpreter,
    mod_name: &str,
    cls_name: &str,
    method: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let m = interp.load_module(mod_name)?;
    let cls = interp.get_attr(&m, cls_name)?;
    let func = interp.get_attr(&cls, method)?;
    call_kw(interp, func, pos, kw)
}

fn is_callable(interp: &mut Interpreter, v: &Value) -> bool {
    if v.is_none() {
        return false;
    }
    call_pos(
        interp,
        Value::builtin_function("callable"),
        std::slice::from_ref(v),
    )
    .and_then(|res| interp.truthy_value(&res))
    .unwrap_or(false)
}

fn now_epoch_f64(interp: &mut Interpreter) -> Result<f64> {
    let time_mod = interp.load_module("time")?;
    let time_fn = interp.get_attr(&time_mod, "time")?;
    let t = call_pos(interp, time_fn, &[])?;
    Ok(value_as_f64(&t).unwrap_or(0.0))
}

fn time_tuple(interp: &mut Interpreter, utc: bool, epoch: i64) -> Result<Value> {
    let time_mod = interp.load_module("time")?;
    let fn_name = if utc { "gmtime" } else { "localtime" };
    let func = interp.get_attr(&time_mod, fn_name)?;
    call_pos(interp, func, &[Value::int(epoch)])
}

fn format_time_tuple(interp: &mut Interpreter, fmt: &str, tt: &Value) -> Result<String> {
    let time_mod = interp.load_module("time")?;
    let strftime = interp.get_attr(&time_mod, "strftime")?;
    let res = call_pos(interp, strftime, &[Value::string(fmt), tt.clone()])?;
    Ok(res.as_str().unwrap_or("").to_string())
}

fn is_recursion_error(err: &PyError) -> bool {
    err.class_name_is("RecursionError")
}

fn is_system_exit_or_kb(err: &PyError) -> bool {
    err.class_name_is("SystemExit") || err.class_name_is("KeyboardInterrupt")
}

fn make_syslog_priority_names() -> Value {
    let mut d = PyDict::default();
    for (k, v) in [
        ("alert", 1i64),
        ("crit", 2),
        ("critical", 2),
        ("debug", 7),
        ("emerg", 0),
        ("err", 3),
        ("error", 3),
        ("info", 6),
        ("notice", 5),
        ("panic", 0),
        ("warn", 4),
        ("warning", 4),
    ] {
        d.insert(PyKey::str_from(k), Value::int(v));
    }
    Value::dict(d)
}

fn make_syslog_facility_names() -> Value {
    let mut d = PyDict::default();
    for (k, v) in [
        ("auth", 4i64),
        ("authpriv", 10),
        ("cron", 9),
        ("daemon", 3),
        ("ftp", 11),
        ("kern", 0),
        ("lpr", 6),
        ("mail", 2),
        ("news", 7),
        ("security", 4),
        ("syslog", 5),
        ("user", 1),
        ("uucp", 8),
        ("local0", 16),
        ("local1", 17),
        ("local2", 18),
        ("local3", 19),
        ("local4", 20),
        ("local5", 21),
        ("local6", 22),
        ("local7", 23),
    ] {
        d.insert(PyKey::str_from(k), Value::int(v));
    }
    Value::dict(d)
}

fn make_syslog_priority_map() -> Value {
    let mut d = PyDict::default();
    for (k, v) in [
        ("DEBUG", "debug"),
        ("INFO", "info"),
        ("WARNING", "warning"),
        ("ERROR", "error"),
        ("CRITICAL", "critical"),
    ] {
        d.insert(PyKey::str_from(k), Value::string(v));
    }
    Value::dict(d)
}

fn base_rotating_init(
    interp: &mut Interpreter,
    self_val: &Value,
    filename: Value,
    mode: &str,
    encoding: Value,
    delay: bool,
    errors: Value,
) -> Result<()> {
    call_class_method(
        interp,
        "logging",
        "FileHandler",
        "__init__",
        &[self_val.clone(), filename],
        &[
            ("mode", Value::string(mode)),
            ("encoding", encoding.clone()),
            ("delay", Value::bool_(delay)),
            ("errors", errors.clone()),
        ],
    )?;
    interp.assign_attr(self_val.clone(), "mode", Value::string(mode))?;
    interp.assign_attr(self_val.clone(), "encoding", encoding)?;
    interp.assign_attr(self_val.clone(), "errors", errors)?;
    interp.assign_attr(self_val.clone(), "namer", Value::none())?;
    interp.assign_attr(self_val.clone(), "rotator", Value::none())?;
    Ok(())
}

fn link_base(
    mod_rc: &Rc<RefCell<PyModule>>,
    cls_name: &str,
    base: &Rc<RefCell<crate::value::PyClass>>,
) {
    let cls_val = mod_rc.borrow().attrs.get(cls_name).cloned();
    if let Some(val) = cls_val
        && let ValueKind::PyClass(cls_rc) = val.kind()
    {
        crate::builtin_modules::macros::link_class_bases(cls_rc, vec![Rc::clone(base)]);
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let logging_mod = interp.load_module("logging")?;
    let handler_val = interp.get_attr(&logging_mod, "Handler")?;
    let file_handler_val = interp.get_attr(&logging_mod, "FileHandler")?;
    let ValueKind::PyClass(handler_cls) = handler_val.kind() else {
        return Err(PyError::runtime(
            "logging.Handler is not a class".to_string(),
        ));
    };
    let ValueKind::PyClass(file_handler_cls) = file_handler_val.kind() else {
        return Err(PyError::runtime(
            "logging.FileHandler is not a class".to_string(),
        ));
    };

    for name in ["BaseRotatingHandler", "WatchedFileHandler"] {
        link_base(module, name, file_handler_cls);
    }
    for name in [
        "BufferingHandler",
        "QueueHandler",
        "SocketHandler",
        "SysLogHandler",
        "HTTPHandler",
        "SMTPHandler",
    ] {
        link_base(module, name, handler_cls);
    }
    Ok(())
}

struct QueueListenerState {
    started: bool,
}

pyrust_module! {
    constants {
        "__package__" => Value::string("logging"),
        "DEFAULT_TCP_LOGGING_PORT" => Value::int(DEFAULT_TCP_LOGGING_PORT),
        "DEFAULT_UDP_LOGGING_PORT" => Value::int(DEFAULT_UDP_LOGGING_PORT),
        "DEFAULT_HTTP_LOGGING_PORT" => Value::int(DEFAULT_HTTP_LOGGING_PORT),
        "DEFAULT_SOAP_LOGGING_PORT" => Value::int(DEFAULT_SOAP_LOGGING_PORT),
        "SYSLOG_UDP_PORT" => Value::int(SYSLOG_UDP_PORT),
        "SYSLOG_TCP_PORT" => Value::int(SYSLOG_TCP_PORT),
    }

    // ── BaseRotatingHandler ──────────────────────────────────────────────────

    class BaseRotatingHandler {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            mode: PyStr<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(false.into())] delay: PyBool,
            #[default(OptionalArg::Omitted)] errors: OptValueRef<'_>,
        ) -> Result<Value> {
            let enc = encoding.as_value().cloned().unwrap_or_else(Value::none);
            let errs = errors.as_value().cloned().unwrap_or_else(Value::none);
            base_rotating_init(_interp, self_val.0, filename.0, mode.0.as_ref(), enc, delay.0, errs)?;
            Ok(Value::none())
        }

        fn emit(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            let run = (|| -> Result<()> {
                let should = call_method(_interp, self_val.0, "shouldRollover", std::slice::from_ref(&record.0))?;
                if _interp.truthy_value(&should)? {
                    call_method(_interp, self_val.0, "doRollover", &[])?;
                }
                call_class_method(
                    _interp,
                    "logging",
                    "FileHandler",
                    "emit",
                    &[self_val.0.clone(), record.0.clone()],
                    &[],
                )?;
                Ok(())
            })();
            if let Err(err) = run {
                if is_recursion_error(&err) || is_system_exit_or_kb(&err) {
                    return Err(err);
                }
                call_method(_interp, self_val.0, "handleError", &[record.0])?;
            }
            Ok(Value::none())
        }

        fn rotation_filename(
            #[positional_only] self_val: PyValueRef<'_>,
            default_name: PyStr<'_>,
        ) -> Result<Value> {
            let namer = _interp.get_attr(self_val.0, "namer").unwrap_or_else(|_| Value::none());
            if is_callable(_interp, &namer) {
                call_pos(_interp, namer, &[Value::string(default_name.0.as_ref())])
            } else {
                Ok(Value::string(default_name.0.as_ref()))
            }
        }

        fn rotate(
            #[positional_only] self_val: PyValueRef<'_>,
            source: PyPath,
            dest: PyPath,
        ) -> Result<Value> {
            let rotator = _interp.get_attr(self_val.0, "rotator").unwrap_or_else(|_| Value::none());
            let src_str = source.as_path().to_string_lossy().into_owned();
            let dst_str = dest.as_path().to_string_lossy().into_owned();
            if is_callable(_interp, &rotator) {
                call_pos(_interp, rotator, &[Value::string(src_str), Value::string(dst_str)])?;
            } else if source.as_path().exists() {
                std::fs::rename(source.as_path(), dest.as_path())
                    .map_err(|e| PyError::from_io_error2(&e, Some(&src_str), Some(&dst_str)))?;
            }
            Ok(Value::none())
        }
    }

    // ── RotatingFileHandler ──────────────────────────────────────────────────

    class RotatingFileHandler : "BaseRotatingHandler" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            #[default(OptionalArg::Omitted)] mode: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] maxBytes: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] backupCount: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(false.into())] delay: PyBool,
            #[default(OptionalArg::Omitted)] errors: OptValueRef<'_>,
        ) -> Result<Value> {
            let max_bytes = maxBytes
                .as_value()
                .and_then(|v| v.as_int())
                .unwrap_or(0);
            let backup_count = backupCount
                .as_value()
                .and_then(|v| v.as_int())
                .unwrap_or(0);
            let mut mode_str = mode
                .as_value()
                .and_then(|v| v.as_str())
                .unwrap_or("a")
                .to_string();
            if max_bytes > 0 {
                mode_str = "a".to_string();
            }
            let enc = encoding.as_value().cloned().unwrap_or_else(Value::none);
            let errs = errors.as_value().cloned().unwrap_or_else(Value::none);
            base_rotating_init(_interp, self_val.0, filename.0, &mode_str, enc, delay.0, errs)?;
            _interp.assign_attr(self_val.0.clone(), "maxBytes", Value::int(max_bytes))?;
            _interp.assign_attr(self_val.0.clone(), "backupCount", Value::int(backup_count))?;
            Ok(Value::none())
        }

        fn doRollover(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let stream = _interp.get_attr(self_val.0, "stream").unwrap_or_else(|_| Value::none());
            if !stream.is_none() {
                call_method(_interp, &stream, "close", &[])?;
                _interp.assign_attr(self_val.0.clone(), "stream", Value::none())?;
            }
            let backup_count = _interp
                .get_attr(self_val.0, "backupCount")?
                .as_int()
                .unwrap_or(0);
            let base_filename = _interp
                .get_attr(self_val.0, "baseFilename")?
                .as_str()
                .unwrap_or("")
                .to_string();
            if backup_count > 0 {
                for i in (1..backup_count).rev() {
                    let sfn_val = call_method(
                        _interp,
                        self_val.0,
                        "rotation_filename",
                        &[Value::string(format!("{base_filename}.{i}"))],
                    )?;
                    let dfn_val = call_method(
                        _interp,
                        self_val.0,
                        "rotation_filename",
                        &[Value::string(format!("{base_filename}.{}", i + 1))],
                    )?;
                    let sfn = sfn_val.as_str().unwrap_or("").to_string();
                    let dfn = dfn_val.as_str().unwrap_or("").to_string();
                    if Path::new(&sfn).exists() {
                        if Path::new(&dfn).exists() {
                            std::fs::remove_file(&dfn)
                                .map_err(|e| PyError::from_io_error(&e, Some(&dfn)))?;
                        }
                        std::fs::rename(&sfn, &dfn)
                            .map_err(|e| PyError::from_io_error2(&e, Some(&sfn), Some(&dfn)))?;
                    }
                }
                let dfn_val = call_method(
                    _interp,
                    self_val.0,
                    "rotation_filename",
                    &[Value::string(format!("{base_filename}.1"))],
                )?;
                let dfn = dfn_val.as_str().unwrap_or("").to_string();
                if Path::new(&dfn).exists() {
                    std::fs::remove_file(&dfn)
                        .map_err(|e| PyError::from_io_error(&e, Some(&dfn)))?;
                }
                call_method(
                    _interp,
                    self_val.0,
                    "rotate",
                    &[Value::string(&base_filename), Value::string(&dfn)],
                )?;
            }
            let delay = _interp
                .get_attr(self_val.0, "delay")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            if !delay {
                let new_stream = call_method(_interp, self_val.0, "_open", &[])?;
                _interp.assign_attr(self_val.0.clone(), "stream", new_stream)?;
            }
            Ok(Value::none())
        }

        fn shouldRollover(
            #[positional_only] self_val: PyValueRef<'_>,
            record: PyValue,
        ) -> Result<Value> {
            let mut stream = _interp.get_attr(self_val.0, "stream").unwrap_or_else(|_| Value::none());
            if stream.is_none() {
                stream = call_method(_interp, self_val.0, "_open", &[])?;
                _interp.assign_attr(self_val.0.clone(), "stream", stream.clone())?;
            }
            let max_bytes = _interp
                .get_attr(self_val.0, "maxBytes")?
                .as_int()
                .unwrap_or(0);
            if max_bytes > 0 {
                let formatted = call_method(_interp, self_val.0, "format", &[record.0])?;
                let msg = format!("{}\n", formatted.as_str().unwrap_or(""));
                call_method(_interp, &stream, "seek", &[Value::int(0), Value::int(2)])?;
                let pos = call_method(_interp, &stream, "tell", &[])?
                    .as_int()
                    .unwrap_or(0);
                if pos + (msg.len() as i64) >= max_bytes {
                    return Ok(Value::int(1));
                }
            }
            Ok(Value::int(0))
        }
    }

    // ── TimedRotatingFileHandler ─────────────────────────────────────────────

    class TimedRotatingFileHandler : "BaseRotatingHandler" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            #[default(OptionalArg::Omitted)] when: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] interval: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] backupCount: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(false.into())] delay: PyBool,
            #[default(false.into())] utc: PyBool,
            #[default(OptionalArg::Omitted)] atTime: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] errors: OptValueRef<'_>,
        ) -> Result<Value> {
            let enc = encoding.as_value().cloned().unwrap_or_else(Value::none);
            let errs = errors.as_value().cloned().unwrap_or_else(Value::none);
            base_rotating_init(_interp, self_val.0, filename.0, "a", enc, delay.0, errs)?;

            let when_raw = when
                .as_value()
                .and_then(|v| v.as_str())
                .unwrap_or("h")
                .to_string();
            let when_upper = when_raw.to_ascii_uppercase();
            let interval_n = interval.as_value().and_then(|v| v.as_int()).unwrap_or(1);
            let backup_count = backupCount.as_value().and_then(|v| v.as_int()).unwrap_or(0);
            let at_time = atTime.as_value().cloned().unwrap_or_else(Value::none);

            let (base_interval, suffix, ext_pat, day_of_week): (i64, &str, &str, Option<i64>) =
                match when_upper.as_str() {
                    "S" => (1, "%Y-%m-%d_%H-%M-%S", r"^\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}(\.\w+)?$", None),
                    "M" => (60, "%Y-%m-%d_%H-%M", r"^\d{4}-\d{2}-\d{2}_\d{2}-\d{2}(\.\w+)?$", None),
                    "H" => (3600, "%Y-%m-%d_%H", r"^\d{4}-\d{2}-\d{2}_\d{2}(\.\w+)?$", None),
                    "D" | "MIDNIGHT" => (86400, "%Y-%m-%d", r"^\d{4}-\d{2}-\d{2}(\.\w+)?$", None),
                    w if w.starts_with('W') => {
                        if w.len() != 2 {
                            return Err(PyError::named(
                                "ValueError",
                                format!("You must specify a day for weekly rollover from 0 to 6 (0 is Monday): {when_upper}"),
                            ));
                        }
                        let ch = w.as_bytes()[1];
                        if !(b'0'..=b'6').contains(&ch) {
                            return Err(PyError::named(
                                "ValueError",
                                format!("Invalid day specified for weekly rollover: {when_upper}"),
                            ));
                        }
                        (
                            86400 * 7,
                            "%Y-%m-%d",
                            r"^\d{4}-\d{2}-\d{2}(\.\w+)?$",
                            Some((ch - b'0') as i64),
                        )
                    }
                    _ => {
                        return Err(PyError::named(
                            "ValueError",
                            format!("Invalid rollover interval specified: {when_upper}"),
                        ));
                    }
                };

            let re_mod = _interp.load_module("re")?;
            let compile_fn = _interp.get_attr(&re_mod, "compile")?;
            let ascii_flag = _interp.get_attr(&re_mod, "ASCII")?;
            let ext_match = call_pos(_interp, compile_fn, &[Value::string(ext_pat), ascii_flag])?;

            _interp.assign_attr(self_val.0.clone(), "when", Value::string(&when_upper))?;
            _interp.assign_attr(self_val.0.clone(), "backupCount", Value::int(backup_count))?;
            _interp.assign_attr(self_val.0.clone(), "utc", Value::bool_(utc.0))?;
            _interp.assign_attr(self_val.0.clone(), "atTime", at_time)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "interval",
                Value::int(base_interval * interval_n),
            )?;
            _interp.assign_attr(self_val.0.clone(), "suffix", Value::string(suffix))?;
            _interp.assign_attr(self_val.0.clone(), "extMatch", ext_match)?;
            if let Some(dow) = day_of_week {
                _interp.assign_attr(self_val.0.clone(), "dayOfWeek", Value::int(dow))?;
            }

            let base_filename = _interp
                .get_attr(self_val.0, "baseFilename")?
                .as_str()
                .unwrap_or("")
                .to_string();
            let t = if Path::new(&base_filename).exists() {
                std::fs::metadata(&base_filename)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|st| st.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or_else(|| now_epoch_f64(_interp).unwrap_or(0.0) as i64)
            } else {
                now_epoch_f64(_interp)? as i64
            };
            let rollover_at = call_method(_interp, self_val.0, "computeRollover", &[Value::int(t)])?;
            _interp.assign_attr(self_val.0.clone(), "rolloverAt", rollover_at)?;
            Ok(Value::none())
        }

        fn computeRollover(
            #[positional_only] self_val: PyValueRef<'_>,
            currentTime: PyValue,
        ) -> Result<Value> {
            let cur = currentTime
                .0
                .as_int()
                .or_else(|| value_as_f64(&currentTime.0).map(|f| f as i64))
                .unwrap_or(0);
            let interval = _interp.get_attr(self_val.0, "interval")?.as_int().unwrap_or(1);
            let when = _interp
                .get_attr(self_val.0, "when")?
                .as_str()
                .unwrap_or("H")
                .to_string();
            let mut result = cur + interval;
            if when == "MIDNIGHT" || when.starts_with('W') {
                let utc = _interp
                    .get_attr(self_val.0, "utc")
                    .ok()
                    .and_then(|v| _interp.truthy_value(&v).ok())
                    .unwrap_or(false);
                let t = time_tuple(_interp, utc, cur)?;
                let cur_h = _interp.get_attr(&t, "tm_hour")?.as_int().unwrap_or(0);
                let cur_m = _interp.get_attr(&t, "tm_min")?.as_int().unwrap_or(0);
                let cur_s = _interp.get_attr(&t, "tm_sec")?.as_int().unwrap_or(0);
                let cur_wday = _interp.get_attr(&t, "tm_wday")?.as_int().unwrap_or(0);
                let at_time = _interp.get_attr(self_val.0, "atTime").unwrap_or_else(|_| Value::none());
                let rotate_ts = if at_time.is_none() {
                    _MIDNIGHT
                } else {
                    let h = _interp.get_attr(&at_time, "hour")?.as_int().unwrap_or(0);
                    let m = _interp.get_attr(&at_time, "minute")?.as_int().unwrap_or(0);
                    let s = _interp.get_attr(&at_time, "second")?.as_int().unwrap_or(0);
                    (h * 60 + m) * 60 + s
                };
                let mut r = rotate_ts - ((cur_h * 60 + cur_m) * 60 + cur_s);
                if r < 0 {
                    r += _MIDNIGHT;
                }
                let mut next = cur + r;
                if when.starts_with('W') {
                    let dow = _interp.get_attr(self_val.0, "dayOfWeek")?.as_int().unwrap_or(0);
                    let mut days_to_wait = dow - cur_wday;
                    if days_to_wait < 0 {
                        days_to_wait += 7;
                    }
                    if days_to_wait == 0 && r < 0 {
                        days_to_wait = 7;
                    }
                    next += days_to_wait * _MIDNIGHT;
                }
                result = next;
            }
            Ok(Value::int(result))
        }

        fn shouldRollover(
            #[positional_only] self_val: PyValueRef<'_>,
            _record: PyValue,
        ) -> Result<Value> {
            let t = now_epoch_f64(_interp)? as i64;
            let rollover_at = _interp
                .get_attr(self_val.0, "rolloverAt")?
                .as_int()
                .unwrap_or(0);
            Ok(Value::int(if t >= rollover_at { 1 } else { 0 }))
        }

        fn getFilesToDelete(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let base_filename = _interp
                .get_attr(self_val.0, "baseFilename")?
                .as_str()
                .unwrap_or("")
                .to_string();
            let p = Path::new(&base_filename);
            let dir = p.parent().unwrap_or_else(|| Path::new("."));
            let base_name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            let prefix = format!("{base_name}.");
            let ext_match = _interp.get_attr(self_val.0, "extMatch")?;
            let backup_count = _interp
                .get_attr(self_val.0, "backupCount")?
                .as_int()
                .unwrap_or(0)
                .max(0) as usize;

            let mut result = Vec::new();
            if let Ok(rd) = std::fs::read_dir(dir) {
                for entry in rd.flatten() {
                    let fname = entry.file_name().to_string_lossy().into_owned();
                    if let Some(suffix) = fname.strip_prefix(&prefix) {
                        let m = call_method(_interp, &ext_match, "match", &[Value::string(suffix)])?;
                        if !m.is_none() {
                            result.push(dir.join(&fname).to_string_lossy().into_owned());
                        }
                    }
                }
            }
            result.sort();
            if result.len() <= backup_count {
                Ok(Value::list(Vec::new()))
            } else {
                let to_del = result.len() - backup_count;
                Ok(Value::list(
                    result.into_iter().take(to_del).map(Value::string).collect(),
                ))
            }
        }

        fn doRollover(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let stream = _interp.get_attr(self_val.0, "stream").unwrap_or_else(|_| Value::none());
            if !stream.is_none() {
                call_method(_interp, &stream, "close", &[])?;
                _interp.assign_attr(self_val.0.clone(), "stream", Value::none())?;
            }
            let cur_time = now_epoch_f64(_interp)? as i64;
            let rollover_at = _interp
                .get_attr(self_val.0, "rolloverAt")?
                .as_int()
                .unwrap_or(cur_time);
            let interval = _interp.get_attr(self_val.0, "interval")?.as_int().unwrap_or(1);
            let utc = _interp
                .get_attr(self_val.0, "utc")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            let suffix = _interp
                .get_attr(self_val.0, "suffix")?
                .as_str()
                .unwrap_or("%Y-%m-%d")
                .to_string();
            let base_filename = _interp
                .get_attr(self_val.0, "baseFilename")?
                .as_str()
                .unwrap_or("")
                .to_string();

            let roll_t = rollover_at - interval;
            let tt = time_tuple(_interp, utc, roll_t)?;
            let stamp = format_time_tuple(_interp, &suffix, &tt)?;
            let dfn_val = call_method(
                _interp,
                self_val.0,
                "rotation_filename",
                &[Value::string(format!("{base_filename}.{stamp}"))],
            )?;
            let dfn = dfn_val.as_str().unwrap_or("").to_string();
            if Path::new(&dfn).exists() {
                std::fs::remove_file(&dfn)
                    .map_err(|e| PyError::from_io_error(&e, Some(&dfn)))?;
            }
            call_method(
                _interp,
                self_val.0,
                "rotate",
                &[Value::string(&base_filename), Value::string(&dfn)],
            )?;

            let backup_count = _interp
                .get_attr(self_val.0, "backupCount")?
                .as_int()
                .unwrap_or(0);
            if backup_count > 0 {
                let files_val = call_method(_interp, self_val.0, "getFilesToDelete", &[])?;
                for f in _interp.collect_iterable(&files_val)? {
                    if let Some(s) = f.as_str() {
                        let _ = std::fs::remove_file(s);
                    }
                }
            }

            let delay = _interp
                .get_attr(self_val.0, "delay")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            if !delay {
                let new_stream = call_method(_interp, self_val.0, "_open", &[])?;
                _interp.assign_attr(self_val.0.clone(), "stream", new_stream)?;
            }

            let mut new_rollover = call_method(
                _interp,
                self_val.0,
                "computeRollover",
                &[Value::int(cur_time)],
            )?
            .as_int()
            .unwrap_or(cur_time + interval);
            while new_rollover <= cur_time {
                new_rollover += interval;
            }
            _interp.assign_attr(self_val.0.clone(), "rolloverAt", Value::int(new_rollover))?;
            Ok(Value::none())
        }
    }

    // ── WatchedFileHandler ───────────────────────────────────────────────────

    class WatchedFileHandler {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            #[default(OptionalArg::Omitted)] mode: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(false.into())] delay: PyBool,
            #[default(OptionalArg::Omitted)] errors: OptValueRef<'_>,
        ) -> Result<Value> {
            let mode_str = mode.as_value().and_then(|v| v.as_str()).unwrap_or("a");
            let enc = encoding.as_value().cloned().unwrap_or_else(Value::none);
            let errs = errors.as_value().cloned().unwrap_or_else(Value::none);
            call_class_method(
                _interp,
                "logging",
                "FileHandler",
                "__init__",
                &[self_val.0.clone(), filename.0],
                &[
                    ("mode", Value::string(mode_str)),
                    ("encoding", enc),
                    ("delay", Value::bool_(delay.0)),
                    ("errors", errs),
                ],
            )?;
            _interp.assign_attr(self_val.0.clone(), "dev", Value::int(-1))?;
            _interp.assign_attr(self_val.0.clone(), "ino", Value::int(-1))?;
            call_method(_interp, self_val.0, "_statstream", &[])?;
            Ok(Value::none())
        }

        fn _statstream(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let stream = _interp.get_attr(self_val.0, "stream").unwrap_or_else(|_| Value::none());
            if !stream.is_none() {
                let base_filename = _interp
                    .get_attr(self_val.0, "baseFilename")?
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let os_mod = _interp.load_module("os")?;
                let stat_fn = _interp.get_attr(&os_mod, "stat")?;
                if let Ok(sres) = call_pos(_interp, stat_fn, &[Value::string(&base_filename)]) {
                    let dev = _interp.get_attr(&sres, "st_dev").unwrap_or_else(|_| Value::int(-1));
                    let ino = _interp.get_attr(&sres, "st_ino").unwrap_or_else(|_| Value::int(-1));
                    _interp.assign_attr(self_val.0.clone(), "dev", dev)?;
                    _interp.assign_attr(self_val.0.clone(), "ino", ino)?;
                }
            }
            Ok(Value::none())
        }

        fn reopenIfNeeded(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let base_filename = _interp
                .get_attr(self_val.0, "baseFilename")?
                .as_str()
                .unwrap_or("")
                .to_string();
            let os_mod = _interp.load_module("os")?;
            let stat_fn = _interp.get_attr(&os_mod, "stat")?;
            let (changed, exists) = match call_pos(_interp, stat_fn, &[Value::string(&base_filename)])
            {
                Ok(sres) => {
                    let cur_dev = _interp.get_attr(&sres, "st_dev")?.as_int().unwrap_or(-1);
                    let cur_ino = _interp.get_attr(&sres, "st_ino")?.as_int().unwrap_or(-1);
                    let old_dev = _interp.get_attr(self_val.0, "dev")?.as_int().unwrap_or(-1);
                    let old_ino = _interp.get_attr(self_val.0, "ino")?.as_int().unwrap_or(-1);
                    (cur_dev != old_dev || cur_ino != old_ino, true)
                }
                Err(_) => (true, false),
            };
            let stream = _interp.get_attr(self_val.0, "stream").unwrap_or_else(|_| Value::none());
            if (!exists || changed) && !stream.is_none() {
                let _ = call_method(_interp, &stream, "flush", &[]);
                let _ = call_method(_interp, &stream, "close", &[]);
                _interp.assign_attr(self_val.0.clone(), "stream", Value::none())?;
                let new_stream = call_method(_interp, self_val.0, "_open", &[])?;
                _interp.assign_attr(self_val.0.clone(), "stream", new_stream)?;
                call_method(_interp, self_val.0, "_statstream", &[])?;
            }
            Ok(Value::none())
        }

        fn emit(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            call_method(_interp, self_val.0, "reopenIfNeeded", &[])?;
            call_class_method(
                _interp,
                "logging",
                "FileHandler",
                "emit",
                &[self_val.0.clone(), record.0],
                &[],
            )
        }
    }

    // ── BufferingHandler & MemoryHandler ─────────────────────────────────────

    class BufferingHandler {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, capacity: PyIndex) -> Result<Value> {
            let cap = capacity.expect_i64(FN_NAME, "capacity")?;
            call_class_method(_interp, "logging", "Handler", "__init__", std::slice::from_ref(self_val.0), &[])?;
            _interp.assign_attr(self_val.0.clone(), "capacity", Value::int(cap))?;
            _interp.assign_attr(self_val.0.clone(), "buffer", Value::list(Vec::new()))?;
            Ok(Value::none())
        }

        fn shouldFlush(
            #[positional_only] self_val: PyValueRef<'_>,
            _record: PyValue,
        ) -> Result<Value> {
            let buf = _interp.get_attr(self_val.0, "buffer")?;
            let len = call_pos(_interp, Value::builtin_function("len"), &[buf])?
                .as_int()
                .unwrap_or(0);
            let cap = _interp.get_attr(self_val.0, "capacity")?.as_int().unwrap_or(0);
            Ok(Value::bool_(len >= cap))
        }

        fn emit(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            let buf = _interp.get_attr(self_val.0, "buffer")?;
            call_method(_interp, &buf, "append", std::slice::from_ref(&record.0))?;
            let should = call_method(_interp, self_val.0, "shouldFlush", &[record.0])?;
            if _interp.truthy_value(&should)? {
                call_method(_interp, self_val.0, "flush", &[])?;
            }
            Ok(Value::none())
        }

        fn flush(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "acquire", &[])?;
            let res = (|| -> Result<()> {
                let buf = _interp.get_attr(self_val.0, "buffer")?;
                call_method(_interp, &buf, "clear", &[])?;
                Ok(())
            })();
            let _ = call_method(_interp, self_val.0, "release", &[]);
            res?;
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let flush_res = call_method(_interp, self_val.0, "flush", &[]);
            let close_res = call_class_method(
                _interp,
                "logging",
                "Handler",
                "close",
                std::slice::from_ref(self_val.0),
                &[],
            );
            flush_res?;
            close_res?;
            Ok(Value::none())
        }
    }

    class MemoryHandler : "BufferingHandler" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            capacity: PyIndex,
            #[default(OptionalArg::Omitted)] flushLevel: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] target: OptValueRef<'_>,
            #[default(true.into())] flushOnClose: PyBool,
        ) -> Result<Value> {
            let cap = capacity.expect_i64(FN_NAME, "capacity")?;
            call_class_method(
                _interp,
                "logging.handlers",
                "BufferingHandler",
                "__init__",
                &[self_val.0.clone(), Value::int(cap)],
                &[],
            )?;
            let level_raw = match flushLevel.as_value() {
                Some(v) => v.clone(),
                None => logging_attr(_interp, "ERROR")?,
            };
            let check_level = logging_attr(_interp, "_checkLevel")?;
            let checked = call_pos(_interp, check_level, &[level_raw])?;
            _interp.assign_attr(self_val.0.clone(), "flushLevel", checked)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "target",
                target.as_value().cloned().unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "flushOnClose",
                Value::bool_(flushOnClose.0),
            )?;
            Ok(Value::none())
        }

        fn shouldFlush(
            #[positional_only] self_val: PyValueRef<'_>,
            record: PyValue,
        ) -> Result<Value> {
            let buf = _interp.get_attr(self_val.0, "buffer")?;
            let len = call_pos(_interp, Value::builtin_function("len"), &[buf])?
                .as_int()
                .unwrap_or(0);
            let cap = _interp.get_attr(self_val.0, "capacity")?.as_int().unwrap_or(0);
            let levelno = _interp.get_attr(&record.0, "levelno")?.as_int().unwrap_or(0);
            let flush_level = _interp.get_attr(self_val.0, "flushLevel")?.as_int().unwrap_or(40);
            Ok(Value::bool_(len >= cap || levelno >= flush_level))
        }

        fn setTarget(#[positional_only] self_val: PyValueRef<'_>, target: PyValue) -> Result<Value> {
            call_method(_interp, self_val.0, "acquire", &[])?;
            let res = _interp.assign_attr(self_val.0.clone(), "target", target.0);
            let _ = call_method(_interp, self_val.0, "release", &[]);
            res?;
            Ok(Value::none())
        }

        fn flush(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "acquire", &[])?;
            let res = (|| -> Result<()> {
                let target = _interp.get_attr(self_val.0, "target")?;
                if !target.is_none() {
                    let buf = _interp.get_attr(self_val.0, "buffer")?;
                    let records = _interp.collect_iterable(&buf)?;
                    for rec in records {
                        call_method(_interp, &target, "handle", &[rec])?;
                    }
                    call_method(_interp, &buf, "clear", &[])?;
                }
                Ok(())
            })();
            let _ = call_method(_interp, self_val.0, "release", &[]);
            res?;
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let flush_on_close = _interp
                .get_attr(self_val.0, "flushOnClose")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(true);
            let flush_res = if flush_on_close {
                call_method(_interp, self_val.0, "flush", &[])
            } else {
                Ok(Value::none())
            };
            call_method(_interp, self_val.0, "acquire", &[])?;
            let close_res = (|| -> Result<()> {
                _interp.assign_attr(self_val.0.clone(), "target", Value::none())?;
                call_class_method(
                    _interp,
                    "logging.handlers",
                    "BufferingHandler",
                    "close",
                    std::slice::from_ref(self_val.0),
                    &[],
                )?;
                Ok(())
            })();
            let _ = call_method(_interp, self_val.0, "release", &[]);
            flush_res?;
            close_res?;
            Ok(Value::none())
        }
    }

    // ── QueueHandler & QueueListener ─────────────────────────────────────────

    class QueueHandler {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, queue: PyValue) -> Result<Value> {
            call_class_method(_interp, "logging", "Handler", "__init__", std::slice::from_ref(self_val.0), &[])?;
            _interp.assign_attr(self_val.0.clone(), "queue", queue.0)?;
            Ok(Value::none())
        }

        fn enqueue(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            let q = _interp.get_attr(self_val.0, "queue")?;
            call_method(_interp, &q, "put_nowait", &[record.0])?;
            Ok(Value::none())
        }

        fn prepare(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            let msg = call_method(_interp, self_val.0, "format", std::slice::from_ref(&record.0))?;
            let copy_mod = _interp.load_module("copy")?;
            let copy_fn = _interp.get_attr(&copy_mod, "copy")?;
            let rec = call_pos(_interp, copy_fn, &[record.0])?;
            _interp.assign_attr(rec.clone(), "message", msg.clone())?;
            _interp.assign_attr(rec.clone(), "msg", msg)?;
            _interp.assign_attr(rec.clone(), "args", Value::none())?;
            _interp.assign_attr(rec.clone(), "exc_info", Value::none())?;
            _interp.assign_attr(rec.clone(), "exc_text", Value::none())?;
            Ok(rec)
        }

        fn emit(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            let run = (|| -> Result<()> {
                let prepared = call_method(_interp, self_val.0, "prepare", std::slice::from_ref(&record.0))?;
                call_method(_interp, self_val.0, "enqueue", &[prepared])?;
                Ok(())
            })();
            if let Err(err) = run {
                if is_recursion_error(&err) || is_system_exit_or_kb(&err) {
                    return Err(err);
                }
                call_method(_interp, self_val.0, "handleError", &[record.0])?;
            }
            Ok(Value::none())
        }
    }

    #[native_state(QueueListenerState)]
    class QueueListener {
        constants {
            "_sentinel" => Value::none(),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            queue: PyValue,
            #[varargs] handlers: &[Value],
            #[default(false.into())] respect_handler_level: PyBool,
        ) -> Result<Value> {
            set_state(self_val.0, QueueListenerState { started: false })?;
            _interp.assign_attr(self_val.0.clone(), "queue", queue.0)?;
            _interp.assign_attr(self_val.0.clone(), "handlers", Value::tuple(handlers.to_vec()))?;
            _interp.assign_attr(self_val.0.clone(), "_thread", Value::none())?;
            _interp.assign_attr(
                self_val.0.clone(),
                "respect_handler_level",
                Value::bool_(respect_handler_level.0),
            )?;
            Ok(Value::none())
        }

        fn dequeue(#[positional_only] self_val: PyValueRef<'_>, block: PyBool) -> Result<Value> {
            let q = _interp.get_attr(self_val.0, "queue")?;
            call_method(_interp, &q, "get", &[Value::bool_(block.0)])
        }

        fn start(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state_mut(self_val.0, |st| {
                st.started = true;
                Ok(())
            })?;
            // In single-threaded PyRust, keep `_thread` non-None while active
            // and drain all queued records synchronously upon `stop()`.
            let threading_mod = _interp.load_module("threading")?;
            let thread_cls = _interp.get_attr(&threading_mod, "Thread")?;
            let monitor = _interp.get_attr(self_val.0, "_monitor")?;
            let t = call_kw(
                _interp,
                thread_cls,
                &[],
                &[("target", monitor), ("daemon", Value::bool_(true))],
            )?;
            _interp.assign_attr(self_val.0.clone(), "_thread", t)?;
            Ok(Value::none())
        }

        fn prepare(#[positional_only] _self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            Ok(record.0)
        }

        fn handle(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            let record = call_method(_interp, self_val.0, "prepare", &[record.0])?;
            let respect = _interp
                .get_attr(self_val.0, "respect_handler_level")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            let handlers_val = _interp.get_attr(self_val.0, "handlers")?;
            for handler in _interp.collect_iterable(&handlers_val)? {
                let process = if !respect {
                    true
                } else {
                    let r_lvl = _interp.get_attr(&record, "levelno")?.as_int().unwrap_or(0);
                    let h_lvl = _interp.get_attr(&handler, "level")?.as_int().unwrap_or(0);
                    r_lvl >= h_lvl
                };
                if process {
                    call_method(_interp, &handler, "handle", std::slice::from_ref(&record))?;
                }
            }
            Ok(Value::none())
        }

        fn _monitor(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let q = _interp.get_attr(self_val.0, "queue")?;
            let has_task_done = _interp.get_attr(&q, "task_done").is_ok();
            let sentinel = _interp.get_attr(self_val.0, "_sentinel")?;
            let queue_mod = _interp.load_module("queue")?;
            let empty_cls = _interp.get_attr(&queue_mod, "Empty")?;

            loop {
                let record = match call_method(_interp, self_val.0, "dequeue", &[Value::bool_(false)]) {
                    Ok(rec) => rec,
                    Err(err) => {
                        if let Ok(exc) = _interp.materialize_pyerror(err.clone())
                            && let Ok(is_empty) = call_pos(
                                _interp,
                                Value::builtin_function("isinstance"),
                                &[exc, empty_cls.clone()],
                            )
                            && _interp.truthy_value(&is_empty).unwrap_or(false)
                        {
                            break;
                        }
                        return Err(err);
                    }
                };
                if values_are_identical(&record, &sentinel) {
                    if has_task_done {
                        let _ = call_method(_interp, &q, "task_done", &[]);
                    }
                    break;
                }
                call_method(_interp, self_val.0, "handle", &[record])?;
                if has_task_done {
                    let _ = call_method(_interp, &q, "task_done", &[]);
                }
            }
            Ok(Value::none())
        }

        fn enqueue_sentinel(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let q = _interp.get_attr(self_val.0, "queue")?;
            let sentinel = _interp.get_attr(self_val.0, "_sentinel")?;
            call_method(_interp, &q, "put_nowait", &[sentinel])?;
            Ok(Value::none())
        }

        fn stop(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "enqueue_sentinel", &[])?;
            call_method(_interp, self_val.0, "_monitor", &[])?;
            with_state_mut(self_val.0, |st| {
                st.started = false;
                Ok(())
            })?;
            _interp.assign_attr(self_val.0.clone(), "_thread", Value::none())?;
            Ok(Value::none())
        }
    }

    // ── SocketHandler & DatagramHandler ──────────────────────────────────────

    class SocketHandler {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyValue,
            port: PyValue,
        ) -> Result<Value> {
            call_class_method(_interp, "logging", "Handler", "__init__", std::slice::from_ref(self_val.0), &[])?;
            _interp.assign_attr(self_val.0.clone(), "host", host.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "port", port.0.clone())?;
            let address = if port.0.is_none() {
                host.0
            } else {
                Value::tuple(vec![host.0, port.0])
            };
            _interp.assign_attr(self_val.0.clone(), "address", address)?;
            _interp.assign_attr(self_val.0.clone(), "sock", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "closeOnError", Value::bool_(false))?;
            _interp.assign_attr(self_val.0.clone(), "retryTime", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "retryStart", Value::float(1.0))?;
            _interp.assign_attr(self_val.0.clone(), "retryMax", Value::float(30.0))?;
            _interp.assign_attr(self_val.0.clone(), "retryFactor", Value::float(2.0))?;
            Ok(Value::none())
        }

        fn makeSocket(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let timeout_val = timeout
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::int(1));
            let socket_mod = _interp.load_module("socket")?;
            let port = _interp.get_attr(self_val.0, "port")?;
            let address = _interp.get_attr(self_val.0, "address")?;
            if !port.is_none() {
                let create_conn = _interp.get_attr(&socket_mod, "create_connection")?;
                call_kw(_interp, create_conn, &[address], &[("timeout", timeout_val)])
            } else {
                let af_unix = _interp.get_attr(&socket_mod, "AF_UNIX")?;
                let sock_stream = _interp.get_attr(&socket_mod, "SOCK_STREAM")?;
                let sock_cls = _interp.get_attr(&socket_mod, "socket")?;
                let s = call_pos(_interp, sock_cls, &[af_unix, sock_stream])?;
                let _ = call_method(_interp, &s, "settimeout", &[timeout_val]);
                call_method(_interp, &s, "connect", &[address])?;
                Ok(s)
            }
        }

        fn createSocket(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let now = now_epoch_f64(_interp)?;
            let retry_time = _interp.get_attr(self_val.0, "retryTime")?;
            let attempt = if retry_time.is_none() {
                true
            } else {
                now >= value_as_f64(&retry_time).unwrap_or(0.0)
            };
            if attempt {
                match call_method(_interp, self_val.0, "makeSocket", &[]) {
                    Ok(sock) => {
                        _interp.assign_attr(self_val.0.clone(), "sock", sock)?;
                        _interp.assign_attr(self_val.0.clone(), "retryTime", Value::none())?;
                    }
                    Err(_) => {
                        let retry_period = match _interp.get_attr(self_val.0, "retryPeriod") {
                            Ok(v) if !v.is_none() => {
                                let cur = value_as_f64(&v).unwrap_or(1.0);
                                let factor_v = _interp.get_attr(self_val.0, "retryFactor")?;
                                let factor = value_as_f64(&factor_v).unwrap_or(2.0);
                                let max_v = _interp.get_attr(self_val.0, "retryMax")?;
                                let max_p = value_as_f64(&max_v).unwrap_or(30.0);
                                (cur * factor).min(max_p)
                            }
                            _ => {
                                let start_v = _interp.get_attr(self_val.0, "retryStart")?;
                                value_as_f64(&start_v).unwrap_or(1.0)
                            }
                        };
                        _interp.assign_attr(
                            self_val.0.clone(),
                            "retryPeriod",
                            Value::float(retry_period),
                        )?;
                        _interp.assign_attr(
                            self_val.0.clone(),
                            "retryTime",
                            Value::float(now + retry_period),
                        )?;
                    }
                }
            }
            Ok(Value::none())
        }

        fn send(#[positional_only] self_val: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            let sock = _interp.get_attr(self_val.0, "sock")?;
            if sock.is_none() {
                call_method(_interp, self_val.0, "createSocket", &[])?;
            }
            let sock = _interp.get_attr(self_val.0, "sock")?;
            if !sock.is_none() && call_method(_interp, &sock, "sendall", &[s.0]).is_err() {
                let _ = call_method(_interp, &sock, "close", &[]);
                _interp.assign_attr(self_val.0.clone(), "sock", Value::none())?;
            }
            Ok(Value::none())
        }

        fn makePickle(
            #[positional_only] self_val: PyValueRef<'_>,
            record: PyValue,
        ) -> Result<Value> {
            let ei = _interp.get_attr(&record.0, "exc_info")?;
            if !ei.is_none() {
                let _ = call_method(_interp, self_val.0, "format", std::slice::from_ref(&record.0));
            }
            let rec_dict = _interp.get_attr(&record.0, "__dict__")?;
            let d = call_method(_interp, &rec_dict, "copy", &[])?;
            let msg = call_method(_interp, &record.0, "getMessage", &[])?;
            call_method(_interp, &d, "__setitem__", &[Value::string("msg"), msg])?;
            call_method(_interp, &d, "__setitem__", &[Value::string("args"), Value::none()])?;
            call_method(_interp, &d, "__setitem__", &[Value::string("exc_info"), Value::none()])?;
            let _ = call_method(_interp, &d, "pop", &[Value::string("message"), Value::none()]);
            let pickle_mod = _interp.load_module("pickle")?;
            let dumps = _interp.get_attr(&pickle_mod, "dumps")?;
            let s = call_pos(_interp, dumps, &[d, Value::int(1)])?;
            let slen = call_pos(_interp, Value::builtin_function("len"), std::slice::from_ref(&s))?
                .as_int()
                .unwrap_or(0) as u32;
            let mut framed = Vec::with_capacity(4 + slen as usize);
            framed.extend_from_slice(&slen.to_be_bytes());
            if let ValueKind::Bytes(b) = s.kind() {
                framed.extend_from_slice(b);
            }
            Ok(Value::bytes(framed))
        }

        fn handleError(
            #[positional_only] self_val: PyValueRef<'_>,
            record: PyValue,
        ) -> Result<Value> {
            let close_on_err = _interp
                .get_attr(self_val.0, "closeOnError")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            let sock = _interp.get_attr(self_val.0, "sock")?;
            if close_on_err && !sock.is_none() {
                let _ = call_method(_interp, &sock, "close", &[]);
                _interp.assign_attr(self_val.0.clone(), "sock", Value::none())?;
            } else {
                call_class_method(
                    _interp,
                    "logging",
                    "Handler",
                    "handleError",
                    &[self_val.0.clone(), record.0],
                    &[],
                )?;
            }
            Ok(Value::none())
        }

        fn emit(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            let run = (|| -> Result<()> {
                let s = call_method(_interp, self_val.0, "makePickle", std::slice::from_ref(&record.0))?;
                call_method(_interp, self_val.0, "send", &[s])?;
                Ok(())
            })();
            if let Err(err) = run {
                if is_recursion_error(&err) || is_system_exit_or_kb(&err) {
                    return Err(err);
                }
                call_method(_interp, self_val.0, "handleError", &[record.0])?;
            }
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "acquire", &[])?;
            let res = (|| -> Result<()> {
                let sock = _interp.get_attr(self_val.0, "sock")?;
                if !sock.is_none() {
                    _interp.assign_attr(self_val.0.clone(), "sock", Value::none())?;
                    let _ = call_method(_interp, &sock, "close", &[]);
                }
                call_class_method(
                    _interp,
                    "logging",
                    "Handler",
                    "close",
                    std::slice::from_ref(self_val.0),
                    &[],
                )?;
                Ok(())
            })();
            let _ = call_method(_interp, self_val.0, "release", &[]);
            res?;
            Ok(Value::none())
        }
    }

    class DatagramHandler : "SocketHandler" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyValue,
            port: PyValue,
        ) -> Result<Value> {
            call_class_method(
                _interp,
                "logging.handlers",
                "SocketHandler",
                "__init__",
                &[self_val.0.clone(), host.0, port.0],
                &[],
            )?;
            _interp.assign_attr(self_val.0.clone(), "closeOnError", Value::bool_(false))?;
            Ok(Value::none())
        }

        fn makeSocket(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let socket_mod = _interp.load_module("socket")?;
            let port = _interp.get_attr(self_val.0, "port")?;
            let sock_dgram = _interp.get_attr(&socket_mod, "SOCK_DGRAM")?;
            let sock_cls = _interp.get_attr(&socket_mod, "socket")?;
            let family = if port.is_none() {
                _interp.get_attr(&socket_mod, "AF_UNIX")?
            } else {
                _interp.get_attr(&socket_mod, "AF_INET")?
            };
            call_pos(_interp, sock_cls, &[family, sock_dgram])
        }

        fn send(#[positional_only] self_val: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            let sock = _interp.get_attr(self_val.0, "sock")?;
            if sock.is_none() {
                call_method(_interp, self_val.0, "createSocket", &[])?;
            }
            let sock = _interp.get_attr(self_val.0, "sock")?;
            let address = _interp.get_attr(self_val.0, "address")?;
            if !sock.is_none() {
                call_method(_interp, &sock, "sendto", &[s.0, address])?;
            }
            Ok(Value::none())
        }
    }

    // ── SysLogHandler ────────────────────────────────────────────────────────

    class SysLogHandler {
        constants {
            "LOG_EMERG" => Value::int(0),
            "LOG_ALERT" => Value::int(1),
            "LOG_CRIT" => Value::int(2),
            "LOG_ERR" => Value::int(3),
            "LOG_WARNING" => Value::int(4),
            "LOG_NOTICE" => Value::int(5),
            "LOG_INFO" => Value::int(6),
            "LOG_DEBUG" => Value::int(7),
            "LOG_KERN" => Value::int(0),
            "LOG_USER" => Value::int(1),
            "LOG_MAIL" => Value::int(2),
            "LOG_DAEMON" => Value::int(3),
            "LOG_AUTH" => Value::int(4),
            "LOG_SYSLOG" => Value::int(5),
            "LOG_LPR" => Value::int(6),
            "LOG_NEWS" => Value::int(7),
            "LOG_UUCP" => Value::int(8),
            "LOG_CRON" => Value::int(9),
            "LOG_AUTHPRIV" => Value::int(10),
            "LOG_FTP" => Value::int(11),
            "LOG_NTP" => Value::int(12),
            "LOG_SECURITY" => Value::int(13),
            "LOG_CONSOLE" => Value::int(14),
            "LOG_SOLCRON" => Value::int(15),
            "LOG_LOCAL0" => Value::int(16),
            "LOG_LOCAL1" => Value::int(17),
            "LOG_LOCAL2" => Value::int(18),
            "LOG_LOCAL3" => Value::int(19),
            "LOG_LOCAL4" => Value::int(20),
            "LOG_LOCAL5" => Value::int(21),
            "LOG_LOCAL6" => Value::int(22),
            "LOG_LOCAL7" => Value::int(23),
            "priority_names" => make_syslog_priority_names(),
            "facility_names" => make_syslog_facility_names(),
            "priority_map" => make_syslog_priority_map(),
            "ident" => Value::string(""),
            "append_nul" => Value::bool_(true),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] address: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] facility: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] socktype: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            call_class_method(_interp, "logging", "Handler", "__init__", std::slice::from_ref(self_val.0), &[])?;
            let addr_val = address.as_value().cloned().unwrap_or_else(|| {
                Value::tuple(vec![Value::string("localhost"), Value::int(SYSLOG_UDP_PORT)])
            });
            let fac_val = facility.as_value().cloned().unwrap_or_else(|| Value::int(1));
            let timeout_val = timeout.as_value().cloned().unwrap_or_else(Value::none);
            let socket_mod = _interp.load_module("socket")?;
            let st_val = socktype
                .as_value()
                .filter(|v| !v.is_none())
                .cloned()
                .unwrap_or_else(|| {
                    _interp
                        .get_attr(&socket_mod, "SOCK_DGRAM")
                        .unwrap_or_else(|_| Value::int(2))
                });

            _interp.assign_attr(self_val.0.clone(), "address", addr_val.clone())?;
            _interp.assign_attr(self_val.0.clone(), "facility", fac_val)?;
            _interp.assign_attr(self_val.0.clone(), "socktype", st_val.clone())?;
            _interp.assign_attr(self_val.0.clone(), "timeout", timeout_val.clone())?;
            _interp.assign_attr(self_val.0.clone(), "socket", Value::none())?;
            let is_unix = addr_val.as_str().is_some();
            _interp.assign_attr(self_val.0.clone(), "unixsocket", Value::bool_(is_unix))?;

            if !is_unix {
                let af_inet = _interp.get_attr(&socket_mod, "AF_INET")?;
                let sock_cls = _interp.get_attr(&socket_mod, "socket")?;
                if let Ok(sock) = call_pos(_interp, sock_cls, &[af_inet, st_val.clone()]) {
                    if !timeout_val.is_none() {
                        let _ = call_method(_interp, &sock, "settimeout", &[timeout_val]);
                    }
                    let sock_stream = _interp
                        .get_attr(&socket_mod, "SOCK_STREAM")
                        .ok()
                        .and_then(|v| v.as_int());
                    if st_val.as_int() == sock_stream {
                        call_method(_interp, &sock, "connect", &[addr_val])?;
                    }
                    _interp.assign_attr(self_val.0.clone(), "socket", sock)?;
                }
            }
            Ok(Value::none())
        }

        fn encodePriority(
            #[positional_only] self_val: PyValueRef<'_>,
            facility: PyValue,
            priority: PyValue,
        ) -> Result<Value> {
            let fac = if let Some(s) = facility.0.as_str() {
                let names = _interp.get_attr(self_val.0, "facility_names")?;
                _interp.eval_index(&names, Value::string(s))?.as_int().unwrap_or(1)
            } else {
                facility.0.as_int().unwrap_or(1)
            };
            let prio = if let Some(s) = priority.0.as_str() {
                let names = _interp.get_attr(self_val.0, "priority_names")?;
                _interp.eval_index(&names, Value::string(s))?.as_int().unwrap_or(4)
            } else {
                priority.0.as_int().unwrap_or(4)
            };
            Ok(Value::int((fac << 3) | prio))
        }

        fn mapPriority(
            #[positional_only] self_val: PyValueRef<'_>,
            levelName: PyStr<'_>,
        ) -> Result<Value> {
            let pmap = _interp.get_attr(self_val.0, "priority_map")?;
            call_method(
                _interp,
                &pmap,
                "get",
                &[Value::string(levelName.0.as_ref()), Value::string("warning")],
            )
        }

        fn emit(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            let run = (|| -> Result<()> {
                let formatted = call_method(_interp, self_val.0, "format", std::slice::from_ref(&record.0))?;
                let mut msg = formatted.as_str().unwrap_or("").to_string();
                let ident = _interp.get_attr(self_val.0, "ident")?;
                if let Some(id_s) = ident.as_str()
                    && !id_s.is_empty()
                {
                    msg = format!("{id_s}{msg}");
                }
                let append_nul = _interp
                    .get_attr(self_val.0, "append_nul")
                    .ok()
                    .and_then(|v| _interp.truthy_value(&v).ok())
                    .unwrap_or(true);
                if append_nul {
                    msg.push('\0');
                }
                let levelname = _interp.get_attr(&record.0, "levelname")?;
                let mapped = call_method(_interp, self_val.0, "mapPriority", &[levelname])?;
                let facility = _interp.get_attr(self_val.0, "facility")?;
                let prio_val = call_method(_interp, self_val.0, "encodePriority", &[facility, mapped])?;
                let prio = prio_val.as_int().unwrap_or(0);
                let payload = format!("<{prio}>{msg}").into_bytes();
                let sock = _interp.get_attr(self_val.0, "socket")?;
                if !sock.is_none() {
                    let socket_mod = _interp.load_module("socket")?;
                    let sock_dgram = _interp.get_attr(&socket_mod, "SOCK_DGRAM")?.as_int();
                    let st = _interp.get_attr(self_val.0, "socktype")?.as_int();
                    if st == sock_dgram {
                        let addr = _interp.get_attr(self_val.0, "address")?;
                        call_method(_interp, &sock, "sendto", &[Value::bytes(payload), addr])?;
                    } else {
                        call_method(_interp, &sock, "sendall", &[Value::bytes(payload)])?;
                    }
                }
                Ok(())
            })();
            if let Err(err) = run {
                if is_recursion_error(&err) || is_system_exit_or_kb(&err) {
                    return Err(err);
                }
                call_method(_interp, self_val.0, "handleError", &[record.0])?;
            }
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "acquire", &[])?;
            let res = (|| -> Result<()> {
                let sock = _interp.get_attr(self_val.0, "socket")?;
                if !sock.is_none() {
                    let _ = call_method(_interp, &sock, "close", &[]);
                    _interp.assign_attr(self_val.0.clone(), "socket", Value::none())?;
                }
                call_class_method(
                    _interp,
                    "logging",
                    "Handler",
                    "close",
                    std::slice::from_ref(self_val.0),
                    &[],
                )?;
                Ok(())
            })();
            let _ = call_method(_interp, self_val.0, "release", &[]);
            res?;
            Ok(Value::none())
        }
    }

    // ── HTTPHandler ──────────────────────────────────────────────────────────

    class HTTPHandler {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyStr<'_>,
            url: PyStr<'_>,
            #[default(OptionalArg::Omitted)] method: OptValueRef<'_>,
            #[default(false.into())] secure: PyBool,
            #[default(OptionalArg::Omitted)] credentials: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] context: OptValueRef<'_>,
        ) -> Result<Value> {
            call_class_method(_interp, "logging", "Handler", "__init__", std::slice::from_ref(self_val.0), &[])?;
            let method_str = method
                .as_value()
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_ascii_uppercase();
            if method_str != "GET" && method_str != "POST" {
                return Err(PyError::named(
                    "ValueError",
                    "method must be GET or POST".to_string(),
                ));
            }
            let ctx = context.as_value().cloned().unwrap_or_else(Value::none);
            if !secure.0 && !ctx.is_none() {
                return Err(PyError::named(
                    "ValueError",
                    "context parameter only makes sense with secure=True".to_string(),
                ));
            }
            _interp.assign_attr(self_val.0.clone(), "host", Value::string(host.0.as_ref()))?;
            _interp.assign_attr(self_val.0.clone(), "url", Value::string(url.0.as_ref()))?;
            _interp.assign_attr(self_val.0.clone(), "method", Value::string(&method_str))?;
            _interp.assign_attr(self_val.0.clone(), "secure", Value::bool_(secure.0))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "credentials",
                credentials.as_value().cloned().unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(self_val.0.clone(), "context", ctx)?;
            Ok(Value::none())
        }

        fn mapLogRecord(
            #[positional_only] _self_val: PyValueRef<'_>,
            record: PyValue,
        ) -> Result<Value> {
            _interp.get_attr(&record.0, "__dict__")
        }

        fn getConnection(
            #[positional_only] self_val: PyValueRef<'_>,
            host: PyValue,
            secure: PyBool,
        ) -> Result<Value> {
            let hc = _interp.load_module("http.client")?;
            if secure.0 {
                let cls = _interp.get_attr(&hc, "HTTPSConnection")?;
                let ctx = _interp.get_attr(self_val.0, "context")?;
                call_kw(_interp, cls, &[host.0], &[("context", ctx)])
            } else {
                let cls = _interp.get_attr(&hc, "HTTPConnection")?;
                call_pos(_interp, cls, &[host.0])
            }
        }

        fn emit(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            let run = (|| -> Result<()> {
                let host = _interp.get_attr(self_val.0, "host")?;
                let secure = _interp.get_attr(self_val.0, "secure")?;
                let h = call_method(_interp, self_val.0, "getConnection", &[host.clone(), secure])?;
                let url_val = _interp.get_attr(self_val.0, "url")?;
                let mut url = url_val.as_str().unwrap_or("").to_string();
                let mapped = call_method(_interp, self_val.0, "mapLogRecord", std::slice::from_ref(&record.0))?;
                let up = _interp.load_module("urllib.parse")?;
                let urlencode = _interp.get_attr(&up, "urlencode")?;
                let data_val = call_pos(_interp, urlencode, &[mapped])?;
                let data = data_val.as_str().unwrap_or("").to_string();
                let method = _interp
                    .get_attr(self_val.0, "method")?
                    .as_str()
                    .unwrap_or("GET")
                    .to_string();
                if method == "GET" {
                    let sep = if url.contains('?') { '&' } else { '?' };
                    url = format!("{url}{sep}{data}");
                }
                call_method(_interp, &h, "putrequest", &[Value::string(&method), Value::string(&url)])?;
                if method == "POST" {
                    call_method(
                        _interp,
                        &h,
                        "putheader",
                        &[
                            Value::string("Content-type"),
                            Value::string("application/x-www-form-urlencoded"),
                        ],
                    )?;
                    call_method(
                        _interp,
                        &h,
                        "putheader",
                        &[Value::string("Content-length"), Value::string(data.len().to_string())],
                    )?;
                }
                call_method(_interp, &h, "endheaders", &[])?;
                if method == "POST" {
                    call_method(_interp, &h, "send", &[Value::bytes(data.into_bytes())])?;
                }
                let _ = call_method(_interp, &h, "getresponse", &[])?;
                Ok(())
            })();
            if let Err(err) = run {
                if is_recursion_error(&err) || is_system_exit_or_kb(&err) {
                    return Err(err);
                }
                call_method(_interp, self_val.0, "handleError", &[record.0])?;
            }
            Ok(Value::none())
        }
    }

    // ── SMTPHandler ──────────────────────────────────────────────────────────

    class SMTPHandler {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            mailhost: PyValue,
            fromaddr: PyStr<'_>,
            toaddrs: PyValue,
            subject: PyStr<'_>,
            #[default(OptionalArg::Omitted)] credentials: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] secure: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            call_class_method(_interp, "logging", "Handler", "__init__", std::slice::from_ref(self_val.0), &[])?;
            if let ValueKind::Tuple(t) = mailhost.0.kind()
                && t.len() == 2
            {
                _interp.assign_attr(self_val.0.clone(), "mailhost", t[0].clone())?;
                _interp.assign_attr(self_val.0.clone(), "mailport", t[1].clone())?;
            } else {
                _interp.assign_attr(self_val.0.clone(), "mailhost", mailhost.0)?;
                _interp.assign_attr(self_val.0.clone(), "mailport", Value::none())?;
            }
            if let Some(creds) = credentials.as_value().filter(|v| !v.is_none()) {
                let parts = _interp.collect_iterable(creds)?;
                if parts.len() == 2 {
                    _interp.assign_attr(self_val.0.clone(), "username", parts[0].clone())?;
                    _interp.assign_attr(self_val.0.clone(), "password", parts[1].clone())?;
                } else {
                    return Err(PyError::named(
                        "ValueError",
                        "credentials must be a 2-tuple (username, password)".to_string(),
                    ));
                }
            } else {
                _interp.assign_attr(self_val.0.clone(), "username", Value::none())?;
            }
            _interp.assign_attr(self_val.0.clone(), "fromaddr", Value::string(fromaddr.0.as_ref()))?;
            let to_list = if toaddrs.0.as_str().is_some() {
                Value::list(vec![toaddrs.0])
            } else {
                toaddrs.0
            };
            _interp.assign_attr(self_val.0.clone(), "toaddrs", to_list)?;
            _interp.assign_attr(self_val.0.clone(), "subject", Value::string(subject.0.as_ref()))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "secure",
                secure.as_value().cloned().unwrap_or_else(Value::none),
            )?;
            let timeout_f = timeout
                .as_value()
                .and_then(value_as_f64)
                .unwrap_or(5.0);
            _interp.assign_attr(self_val.0.clone(), "timeout", Value::float(timeout_f))?;
            Ok(Value::none())
        }

        fn getSubject(
            #[positional_only] self_val: PyValueRef<'_>,
            _record: PyValue,
        ) -> Result<Value> {
            _interp.get_attr(self_val.0, "subject")
        }

        fn emit(#[positional_only] self_val: PyValueRef<'_>, record: PyValue) -> Result<Value> {
            let run = (|| -> Result<()> {
                let smtplib = _interp.load_module("smtplib")?;
                let smtp_cls = _interp.get_attr(&smtplib, "SMTP")?;
                let host = _interp.get_attr(self_val.0, "mailhost")?;
                let port = _interp.get_attr(self_val.0, "mailport")?;
                let port_val = if port.is_none() {
                    _interp
                        .get_attr(&smtplib, "SMTP_PORT")
                        .unwrap_or_else(|_| Value::int(25))
                } else {
                    port
                };
                let timeout = _interp.get_attr(self_val.0, "timeout")?;
                let smtp = call_kw(_interp, smtp_cls, &[host, port_val], &[("timeout", timeout)])?;
                let em_mod = _interp.load_module("email.message")?;
                let msg_cls = _interp.get_attr(&em_mod, "EmailMessage")?;
                let msg = call_pos(_interp, msg_cls, &[])?;
                let fromaddr = _interp.get_attr(self_val.0, "fromaddr")?;
                let toaddrs = _interp.get_attr(self_val.0, "toaddrs")?;
                let to_joined = _interp
                    .collect_iterable(&toaddrs)?
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
                    .join(",");
                let subj = call_method(_interp, self_val.0, "getSubject", std::slice::from_ref(&record.0))?;
                call_method(_interp, &msg, "__setitem__", &[Value::string("From"), fromaddr])?;
                call_method(
                    _interp,
                    &msg,
                    "__setitem__",
                    &[Value::string("To"), Value::string(to_joined)],
                )?;
                call_method(_interp, &msg, "__setitem__", &[Value::string("Subject"), subj])?;
                let body = call_method(_interp, self_val.0, "format", std::slice::from_ref(&record.0))?;
                call_method(_interp, &msg, "set_content", &[body])?;
                call_method(_interp, &smtp, "send_message", &[msg])?;
                let _ = call_method(_interp, &smtp, "quit", &[]);
                Ok(())
            })();
            if let Err(err) = run {
                if is_recursion_error(&err) || is_system_exit_or_kb(&err) {
                    return Err(err);
                }
                call_method(_interp, self_val.0, "handleError", &[record.0])?;
            }
            Ok(Value::none())
        }
    }
}
