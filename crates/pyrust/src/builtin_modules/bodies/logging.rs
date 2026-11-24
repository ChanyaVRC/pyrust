// `logging` module — body for the `logging @inject` entry in
// `pyrust_builtin_modules!` (issue #3151).
//
// CPython 3.12's `logging` is a pure-Python module (`logging_py.py`) built on
// `sys`, `os`, `io`, `time`, `traceback`, `warnings`, `re`, `string`,
// `collections.abc`, and `types`.
//
// One private native helper (`_open_log_file`) backs `FileHandler._open()` with
// a real `std::fs::File`-backed stream so `FileHandler` creates files
// immediately when `delay=False`, flushes writes to disk on every `flush()`,
// supports `encoding` + `errors` (including `backslashreplace`), and releases
// OS file handles deterministically on `close()` (including on Windows).
//
// Reference: <https://docs.python.org/3/library/logging.html>

use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps};
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyKey, Value, ValueKind};

/// Python-source definitions for the `logging` module.
const LOGGING_PY_SOURCE: &str = include_str!("logging_py.py");

/// Private native helper moved into the exec namespace and hidden from
/// `dir(logging)` so the module surface matches CPython 3.12.
const LOGGING_NATIVE_HELPERS: [&str; 1] = ["_open_log_file"];

/// Public and conventional module attributes exported from `LOGGING_PY_SOURCE`
/// onto the `logging` module.
const LOGGING_PY_EXPORTS: &[&str] = &[
    "__all__",
    "__author__",
    "__status__",
    "__version__",
    "__date__",
    "BASIC_FORMAT",
    "CRITICAL",
    "FATAL",
    "ERROR",
    "WARNING",
    "WARN",
    "INFO",
    "DEBUG",
    "NOTSET",
    "raiseExceptions",
    "logThreads",
    "logMultiprocessing",
    "logProcesses",
    "logAsyncioTasks",
    "_startTime",
    "_levelToName",
    "_nameToLevel",
    "_srcfile",
    "_STYLES",
    "_defaultFormatter",
    "_defaultLastResort",
    "_handlers",
    "_handlerList",
    "_loggerClass",
    "_logRecordFactory",
    "getLevelNamesMapping",
    "getLevelName",
    "addLevelName",
    "currentframe",
    "_is_internal_frame",
    "_checkLevel",
    "_acquireLock",
    "_releaseLock",
    "LogRecord",
    "setLogRecordFactory",
    "getLogRecordFactory",
    "makeLogRecord",
    "PercentStyle",
    "StrFormatStyle",
    "StringTemplateStyle",
    "Formatter",
    "BufferingFormatter",
    "Filter",
    "Filterer",
    "getHandlerByName",
    "getHandlerNames",
    "Handler",
    "StreamHandler",
    "FileHandler",
    "NullHandler",
    "_StderrHandler",
    "lastResort",
    "PlaceHolder",
    "setLoggerClass",
    "getLoggerClass",
    "Manager",
    "Logger",
    "RootLogger",
    "LoggerAdapter",
    "root",
    "basicConfig",
    "getLogger",
    "critical",
    "fatal",
    "error",
    "exception",
    "warning",
    "warn",
    "info",
    "debug",
    "log",
    "disable",
    "shutdown",
    "captureWarnings",
];

/// Execute `LOGGING_PY_SOURCE` once with a distinct frozen filename so
/// `Logger.findCaller()` never mistakes user `<string>` / `exec()` frames for
/// internal `logging` frames, then publish the module surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in LOGGING_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source_with_filename(
        LOGGING_PY_SOURCE,
        "<frozen logging>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("logging: exec namespace not a dict".into()))?;
    for name in LOGGING_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("logging"));
            }
            module
                .borrow_mut()
                .attrs
                .insert((*name).to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Real `std::fs::File`-backed stream for `FileHandler` ─────────────────────

struct LogFileState {
    file: Option<std::fs::File>,
    path: String,
    mode: String,
    is_binary: bool,
    is_readable: bool,
    is_writable: bool,
    encoding: Option<String>,
    encoding_display: Option<String>,
    errors: Option<String>,
    closed: bool,
}

struct LogFileOps;

const LOG_FILE_OPS: &LogFileOps = &LogFileOps;
const LOG_FILE_TYPE_NAME: &str = "_io.TextIOWrapper";

fn closed_file_error() -> PyError {
    PyError::named("ValueError", "I/O operation on closed file.".to_string())
}

fn normalise_encoding(enc: &str) -> String {
    enc.to_ascii_lowercase().replace('_', "-")
}

fn validate_encoding(enc: &str) -> Result<String> {
    let norm = normalise_encoding(enc);
    match norm.as_str() {
        "locale" | "utf-8" | "utf8" | "u8" | "utf" | "utf-8-sig" | "utf8sig" | "utf-8sig"
        | "ascii" | "us-ascii" | "646" | "latin-1" | "iso-8859-1" | "8859" | "cp819" | "latin1"
        | "l1" | "utf-16" | "utf16" => Ok(norm),
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown encoding: {enc}"),
        )),
    }
}

fn backslash_escape_char(ch: char) -> String {
    let cp = ch as u32;
    if cp <= 0xff {
        format!("\\x{cp:02x}")
    } else if cp <= 0xffff {
        format!("\\u{cp:04x}")
    } else {
        format!("\\U{cp:08x}")
    }
}

fn encode_text(text: &str, enc: &str, errors: &str) -> Result<Vec<u8>> {
    match enc {
        "locale" | "utf-8" | "utf8" | "u8" | "utf" => Ok(text.as_bytes().to_vec()),
        "utf-8-sig" | "utf8sig" | "utf-8sig" => Ok(text.as_bytes().to_vec()),
        "utf-16" | "utf16" => {
            let mut out = Vec::with_capacity(text.len() * 2);
            for unit in text.encode_utf16() {
                out.extend_from_slice(&unit.to_le_bytes());
            }
            Ok(out)
        }
        "ascii" | "us-ascii" | "646" => {
            let mut out = Vec::with_capacity(text.len());
            for ch in text.chars() {
                if (ch as u32) < 128 {
                    out.push(ch as u8);
                } else {
                    match errors {
                        "ignore" => {}
                        "replace" => out.push(b'?'),
                        "backslashreplace" => {
                            out.extend_from_slice(backslash_escape_char(ch).as_bytes())
                        }
                        "xmlcharrefreplace" => {
                            out.extend_from_slice(format!("&#{};", ch as u32).as_bytes());
                        }
                        _ => {
                            return Err(PyError::named(
                                "UnicodeEncodeError",
                                "'ascii' codec can't encode character: ordinal not in range(128)"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
            Ok(out)
        }
        "latin-1" | "iso-8859-1" | "8859" | "cp819" | "latin1" | "l1" => {
            let mut out = Vec::with_capacity(text.len());
            for ch in text.chars() {
                if (ch as u32) <= 255 {
                    out.push(ch as u8);
                } else {
                    match errors {
                        "ignore" => {}
                        "replace" => out.push(b'?'),
                        "backslashreplace" => {
                            out.extend_from_slice(backslash_escape_char(ch).as_bytes())
                        }
                        "xmlcharrefreplace" => {
                            out.extend_from_slice(format!("&#{};", ch as u32).as_bytes());
                        }
                        _ => {
                            return Err(PyError::named(
                                "UnicodeEncodeError",
                                "'latin-1' codec can't encode character: ordinal not in range(256)"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
            Ok(out)
        }
        _ => Ok(text.as_bytes().to_vec()),
    }
}

fn positional_or_kw<'a>(args: &'a [ExpandedCallArg], pos: usize, name: &str) -> Option<&'a Value> {
    args.iter()
        .find(|a| a.name.as_deref() == Some(name))
        .map(|a| &a.value)
        .or_else(|| {
            args.iter()
                .filter(|a| a.name.is_none())
                .nth(pos)
                .map(|a| &a.value)
        })
}

fn open_log_file_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    let path_val = positional_or_kw(args, 0, "file").ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!("{fn_name}() missing required argument: 'file'"),
        )
    })?;
    let path = match path_val.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}() path must be str, not {}",
                    value_type_name_str(path_val),
                ),
            ));
        }
    };
    let mode = match positional_or_kw(args, 1, "mode") {
        None => "r".to_string(),
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{fn_name}() argument 'mode' must be str, not {}",
                        value_type_name_str(v),
                    ),
                ));
            }
        },
    };
    let encoding_opt = match positional_or_kw(args, 2, "encoding") {
        None => None,
        Some(v) => match v.kind() {
            ValueKind::None => None,
            ValueKind::Str(s) => Some(s.to_string()),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "open() argument 'encoding' must be str or None, not {}",
                        value_type_name_str(v),
                    ),
                ));
            }
        },
    };
    let errors_opt = match positional_or_kw(args, 3, "errors") {
        None => None,
        Some(v) => match v.kind() {
            ValueKind::None => None,
            ValueKind::Str(s) => Some(s.to_string()),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "open() argument 'errors' must be str or None, not {}",
                        value_type_name_str(v),
                    ),
                ));
            }
        },
    };

    let mut seen_rwax = 0usize;
    let mut is_binary = false;
    let mut is_text = false;
    let mut has_plus = false;
    for ch in mode.chars() {
        match ch {
            'r' | 'w' | 'a' | 'x' => seen_rwax += 1,
            'b' => is_binary = true,
            't' => is_text = true,
            '+' => has_plus = true,
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    format!("invalid mode: '{mode}'"),
                ));
            }
        }
    }
    if seen_rwax != 1 || (is_binary && is_text) {
        return Err(PyError::named(
            "ValueError",
            format!("invalid mode: '{mode}'"),
        ));
    }

    let (encoding_norm, encoding_display, errors_str) = if is_binary {
        if encoding_opt.is_some() {
            return Err(PyError::named(
                "ValueError",
                "binary mode doesn't take an encoding argument".to_string(),
            ));
        }
        if errors_opt.is_some() {
            return Err(PyError::named(
                "ValueError",
                "binary mode doesn't take an errors argument".to_string(),
            ));
        }
        (None, None, None)
    } else {
        let (norm, disp) = match encoding_opt {
            Some(enc) => {
                let norm = validate_encoding(&enc)?;
                let disp = if norm == "locale" {
                    "utf-8".to_string()
                } else {
                    enc
                };
                (Some(norm), Some(disp))
            }
            None => (Some("utf-8".to_string()), Some("utf-8".to_string())),
        };
        let errs = Some(errors_opt.unwrap_or_else(|| "strict".to_string()));
        (norm, disp, errs)
    };

    let is_readable = mode.contains('r') || has_plus;
    let is_writable = mode.contains('w') || mode.contains('a') || mode.contains('x') || has_plus;

    let mut opts = OpenOptions::new();
    if mode.contains('r') {
        opts.read(true);
        if has_plus {
            opts.write(true);
        }
    } else if mode.contains('w') {
        opts.write(true).create(true).truncate(true);
        if has_plus {
            opts.read(true);
        }
    } else if mode.contains('a') {
        opts.write(true).create(true).append(true);
        if has_plus {
            opts.read(true);
        }
    } else if mode.contains('x') {
        opts.write(true).create_new(true);
        if has_plus {
            opts.read(true);
        }
    }

    let file = opts
        .open(&path)
        .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;

    let state = LogFileState {
        file: Some(file),
        path,
        mode,
        is_binary,
        is_readable,
        is_writable,
        encoding: encoding_norm,
        encoding_display,
        errors: errors_str,
        closed: false,
    };
    Ok(Value::builtin_object(LOG_FILE_OPS, Box::new(state)))
}

fn log_file_has_method(name: &str) -> bool {
    matches!(
        name,
        "__enter__"
            | "__exit__"
            | "close"
            | "flush"
            | "isatty"
            | "readable"
            | "writable"
            | "seekable"
            | "read"
            | "seek"
            | "tell"
            | "truncate"
            | "write"
            | "writelines"
    )
}

impl BuiltinTypeOps for LogFileOps {
    fn type_name(&self) -> &'static str {
        LOG_FILE_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        if let Some(s) = borrow.downcast_ref::<LogFileState>() {
            if s.is_binary {
                format!("<_io.BufferedWriter name='{}'>", s.path)
            } else {
                let enc = s.encoding_display.as_deref().unwrap_or("utf-8");
                format!(
                    "<_io.TextIOWrapper name='{}' mode='{}' encoding='{}'>",
                    s.path, s.mode, enc
                )
            }
        } else {
            "<log file object>".to_string()
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<LogFileState>()?;
        match name {
            "name" => Some(Value::string(s.path.clone())),
            "mode" => Some(Value::string(s.mode.clone())),
            "closed" => Some(Value::bool_(s.closed)),
            "encoding" => s
                .encoding_display
                .as_ref()
                .map(|enc| Value::string(enc.clone())),
            "errors" => s.errors.as_ref().map(|e| Value::string(e.clone())),
            _ => None,
        }
    }

    fn has_method(&self, name: &str) -> bool {
        log_file_has_method(name)
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        log_file_call_method(state, name, &args)
    }
}

fn log_file_close(state: &BuiltinState) -> Result<()> {
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<LogFileState>()
        .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
    if !s.closed {
        if let Some(mut file) = s.file.take() {
            let _ = file.flush();
            drop(file);
        }
        s.closed = true;
    }
    Ok(())
}

fn log_file_write_one(s: &mut LogFileState, val: &Value) -> Result<i64> {
    if !s.is_writable {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not writable".to_string(),
        ));
    }
    let path = s.path.clone();
    let is_binary = s.is_binary;
    let enc = s.encoding.clone().unwrap_or_else(|| "utf-8".to_string());
    let errs = s.errors.clone().unwrap_or_else(|| "strict".to_string());
    let file = s.file.as_mut().ok_or_else(closed_file_error)?;

    if is_binary {
        let bytes: Vec<u8> = if let ValueKind::Bytes(b) = val.kind() {
            b.as_slice().to_vec()
        } else if let Some(ba) = pyrust_builtins::bytearray::as_bytearray_rc(val) {
            ba.borrow().clone()
        } else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    value_type_name_str(val)
                ),
            ));
        };
        file.write_all(&bytes)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        Ok(bytes.len() as i64)
    } else {
        let text = match val.kind() {
            ValueKind::Str(text) => text.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "write() argument must be str, not {}",
                        value_type_name_str(val)
                    ),
                ));
            }
        };
        let char_len = text.chars().count() as i64;
        let encoded = encode_text(&text, &enc, &errs)?;
        file.write_all(&encoded)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        Ok(char_len)
    }
}

fn log_file_call_method(state: &BuiltinState, method: &str, args: &[Value]) -> Result<Value> {
    match method {
        "__enter__" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::builtin_object_shared(LOG_FILE_OPS, Rc::clone(state)))
        }
        "__exit__" => {
            log_file_close(state)?;
            Ok(Value::none())
        }
        "close" => {
            log_file_close(state)?;
            Ok(Value::none())
        }
        "flush" => {
            let mut borrow = state.borrow_mut();
            let s = borrow
                .downcast_mut::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            let path = s.path.clone();
            let file = s.file.as_mut().ok_or_else(closed_file_error)?;
            file.flush()
                .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
            Ok(Value::none())
        }
        "isatty" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(false))
        }
        "readable" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(s.is_readable))
        }
        "writable" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(s.is_writable))
        }
        "seekable" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(true))
        }
        "tell" => {
            let mut borrow = state.borrow_mut();
            let s = borrow
                .downcast_mut::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            let path = s.path.clone();
            let file = s.file.as_mut().ok_or_else(closed_file_error)?;
            let pos = file
                .stream_position()
                .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
            Ok(Value::int(pos as i64))
        }
        "seek" => {
            let offset = match args.first().map(|v| v.kind()) {
                Some(ValueKind::Int(n)) => n,
                Some(ValueKind::Bool(b)) => b as i64,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "seek() argument must be int".to_string(),
                    ));
                }
            };
            let whence = match args.get(1).map(|v| v.kind()) {
                None => 0,
                Some(ValueKind::Int(n)) => n,
                Some(ValueKind::Bool(b)) => b as i64,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "seek() whence must be int".to_string(),
                    ));
                }
            };
            let mut borrow = state.borrow_mut();
            let s = borrow
                .downcast_mut::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            let path = s.path.clone();
            let file = s.file.as_mut().ok_or_else(closed_file_error)?;
            let seek_from = match whence {
                0 => {
                    if offset < 0 {
                        return Err(PyError::named(
                            "ValueError",
                            format!("negative seek position {offset}"),
                        ));
                    }
                    SeekFrom::Start(offset as u64)
                }
                1 => SeekFrom::Current(offset),
                2 => SeekFrom::End(offset),
                _ => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("invalid whence ({whence}, should be 0, 1 or 2)"),
                    ));
                }
            };
            let new_pos = file
                .seek(seek_from)
                .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
            Ok(Value::int(new_pos as i64))
        }
        "truncate" => {
            let size_opt = match args.first().map(|v| v.kind()) {
                None | Some(ValueKind::None) => None,
                Some(ValueKind::Int(n)) => {
                    if n < 0 {
                        return Err(PyError::named(
                            "ValueError",
                            format!("negative size value {n}"),
                        ));
                    }
                    Some(n as u64)
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "truncate() argument must be int or None".to_string(),
                    ));
                }
            };
            let mut borrow = state.borrow_mut();
            let s = borrow
                .downcast_mut::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            if !s.is_writable {
                return Err(PyError::named(
                    "io.UnsupportedOperation",
                    "File not open for writing".to_string(),
                ));
            }
            let path = s.path.clone();
            let file = s.file.as_mut().ok_or_else(closed_file_error)?;
            let target = match size_opt {
                Some(sz) => sz,
                None => file
                    .stream_position()
                    .map_err(|e| PyError::from_io_error(&e, Some(&path)))?,
            };
            file.set_len(target)
                .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
            Ok(Value::int(target as i64))
        }
        "read" => {
            let mut borrow = state.borrow_mut();
            let s = borrow
                .downcast_mut::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            if !s.is_readable {
                return Err(PyError::named(
                    "io.UnsupportedOperation",
                    "not readable".to_string(),
                ));
            }
            let path = s.path.clone();
            let is_binary = s.is_binary;
            let file = s.file.as_mut().ok_or_else(closed_file_error)?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)
                .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
            if is_binary {
                Ok(Value::bytes(buf))
            } else {
                let text = String::from_utf8_lossy(&buf).into_owned();
                Ok(Value::string(text))
            }
        }
        "write" => {
            let Some(arg) = args.first() else {
                return Err(PyError::named(
                    "TypeError",
                    "write() takes exactly one argument (0 given)".to_string(),
                ));
            };
            let mut borrow = state.borrow_mut();
            let s = borrow
                .downcast_mut::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            let written = log_file_write_one(s, arg)?;
            Ok(Value::int(written))
        }
        "writelines" => {
            let Some(lines) = args.first() else {
                return Err(PyError::named(
                    "TypeError",
                    "writelines() takes exactly one argument (0 given)".to_string(),
                ));
            };
            let items: Vec<Value> = match lines.kind() {
                ValueKind::List(l) => l.iter().cloned().collect(),
                ValueKind::Tuple(t) => t.to_vec(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "writelines() argument must be a sequence".to_string(),
                    ));
                }
            };
            let mut borrow = state.borrow_mut();
            let s = borrow
                .downcast_mut::<LogFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad log file state".to_string()))?;
            for item in &items {
                log_file_write_one(s, item)?;
            }
            Ok(Value::none())
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'{LOG_FILE_TYPE_NAME}' object has no attribute '{method}'"),
        )),
    }
}

pyrust_module! {
    /// Open a real `std::fs::File`-backed log stream for `FileHandler`.
    fn _open_log_file(args) -> Result<Value> {
        let _ = _interp;
        open_log_file_impl(FN_NAME, args)
    }
}
