// `tempfile` module — body for the `tempfile @inject` entry in
// `pyrust_builtin_modules!` (issue #3148).
//
// Native Rust helpers provide atomic exclusive file creation (`create_new`,
// 0o600 + O_NOFOLLOW on Unix) and a real `std::fs::File`-backed seekable
// read/write stream (`TempFileOps`) so `TemporaryFile`, `NamedTemporaryFile`,
// and rolled-over `SpooledTemporaryFile` support full `w+b`/`w+` I/O and
// deterministic handle-close-before-unlink cleanup on both Windows and Linux.
//
// Reference: <https://docs.python.org/3/library/tempfile.html>

use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom, Write};
use std::rc::Rc;
use std::sync::atomic::{AtomicI64, Ordering};

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps};
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, reject_keyword_args_expanded, value_type_name_str,
};
use crate::value::{PyKey, Value, ValueKind};

/// Python-source definitions for the `tempfile` module.
const TEMPFILE_PY_SOURCE: &str = include_str!("tempfile_py.py");

/// Private native helpers moved into the exec namespace and hidden from
/// `dir(tempfile)` so the module surface matches CPython 3.12.
const TEMPFILE_NATIVE_HELPERS: [&str; 2] = ["_mkstemp_open", "_open_native_temp"];

/// Public attributes exported onto the `tempfile` module.
const TEMPFILE_PY_EXPORTS: [&str; 14] = [
    "TMP_MAX",
    "template",
    "tempdir",
    "gettempprefix",
    "gettempprefixb",
    "gettempdir",
    "gettempdirb",
    "mkstemp",
    "mkdtemp",
    "mktemp",
    "NamedTemporaryFile",
    "TemporaryFile",
    "SpooledTemporaryFile",
    "TemporaryDirectory",
];

static NEXT_TEMP_FILENO: AtomicI64 = AtomicI64::new(1000);

/// Execute `TEMPFILE_PY_SOURCE` once, wire its wrappers to the native helpers,
/// hide private helper names from `dir(tempfile)`, and export the public
/// CPython 3.12 `tempfile` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in TEMPFILE_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(TEMPFILE_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("tempfile: exec namespace not a dict".into()))?;
    for name in TEMPFILE_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("tempfile"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Exclusive OS file creation ──────────────────────────────────────────────

fn create_exclusive_file(path: &str) -> Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(path)
        .map_err(|e| PyError::from_io_error(&e, Some(path)))
}

fn require_str(fn_name: &str, value: &Value, what: &str) -> Result<String> {
    match value.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() {what} must be str, not {}",
                value_type_name_str(value),
            ),
        )),
    }
}

fn mkstemp_open_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let path = require_str(fn_name, &args[0].value, "path")?;
    let file = create_exclusive_file(&path)?;
    let fd = super::os::register_fd(file);
    Ok(Value::int(fd))
}

// ── Real `std::fs::File`-backed temporary stream ────────────────────────────

struct TempFileState {
    file: Option<std::fs::File>,
    fd: i64,
    path: String,
    display_name: Value,
    mode: String,
    is_binary: bool,
    is_readable: bool,
    is_writable: bool,
    is_append: bool,
    encoding: Option<String>,
    encoding_display: Option<String>,
    errors: Option<String>,
    newline: Option<String>,
    closed: bool,
}

struct TempFileOps;

const TEMP_FILE_OPS: &TempFileOps = &TempFileOps;
const TEMP_FILE_TYPE_NAME: &str = "_io.BufferedRandom";

fn closed_file_error() -> PyError {
    PyError::named("ValueError", "I/O operation on closed file.".to_string())
}

fn normalise_encoding(enc: &str) -> String {
    enc.to_ascii_lowercase().replace('_', "-")
}

fn validate_encoding(enc: &str) -> Result<String> {
    let norm = normalise_encoding(enc);
    match norm.as_str() {
        "utf-8" | "utf8" | "u8" | "utf" | "ascii" | "us-ascii" | "646" | "latin-1"
        | "iso-8859-1" | "8859" | "cp819" | "latin1" | "l1" => Ok(norm),
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown encoding: {enc}"),
        )),
    }
}

fn encode_text(text: &str, enc: &str, errors: &str) -> Result<Vec<u8>> {
    match enc {
        "utf-8" | "utf8" | "u8" | "utf" => Ok(text.as_bytes().to_vec()),
        "ascii" | "us-ascii" | "646" => {
            let mut out = Vec::with_capacity(text.len());
            for ch in text.chars() {
                if (ch as u32) < 128 {
                    out.push(ch as u8);
                } else if errors == "ignore" {
                    continue;
                } else if errors == "replace" {
                    out.push(b'?');
                } else {
                    return Err(PyError::named(
                        "UnicodeEncodeError",
                        "'ascii' codec can't encode character: ordinal not in range(128)"
                            .to_string(),
                    ));
                }
            }
            Ok(out)
        }
        "latin-1" | "iso-8859-1" | "8859" | "cp819" | "latin1" | "l1" => {
            let mut out = Vec::with_capacity(text.len());
            for ch in text.chars() {
                if (ch as u32) <= 255 {
                    out.push(ch as u8);
                } else if errors == "ignore" {
                    continue;
                } else if errors == "replace" {
                    out.push(b'?');
                } else {
                    return Err(PyError::named(
                        "UnicodeEncodeError",
                        "'latin-1' codec can't encode character: ordinal not in range(256)"
                            .to_string(),
                    ));
                }
            }
            Ok(out)
        }
        _ => Ok(text.as_bytes().to_vec()),
    }
}

fn decode_text(raw: &[u8], enc: &str, errors: &str) -> Result<String> {
    match enc {
        "utf-8" | "utf8" | "u8" | "utf" => match std::str::from_utf8(raw) {
            Ok(s) => Ok(s.to_string()),
            Err(_) if errors == "replace" => Ok(String::from_utf8_lossy(raw).into_owned()),
            Err(_) if errors == "ignore" => {
                let mut out = String::new();
                let mut slice = raw;
                while !slice.is_empty() {
                    match std::str::from_utf8(slice) {
                        Ok(valid) => {
                            out.push_str(valid);
                            break;
                        }
                        Err(err) => {
                            let (valid, after) = slice.split_at(err.valid_up_to());
                            out.push_str(
                                std::str::from_utf8(valid).expect("prefix is valid utf-8"),
                            );
                            let skip = err.error_len().unwrap_or(after.len());
                            slice = &after[skip..];
                        }
                    }
                }
                Ok(out)
            }
            Err(_) => Err(PyError::named(
                "UnicodeDecodeError",
                "'utf-8' codec can't decode bytes: invalid byte sequence".to_string(),
            )),
        },
        "ascii" | "us-ascii" | "646" => {
            let mut out = String::with_capacity(raw.len());
            for &b in raw {
                if b < 128 {
                    out.push(b as char);
                } else if errors == "ignore" {
                    continue;
                } else if errors == "replace" {
                    out.push('\u{FFFD}');
                } else {
                    return Err(PyError::named(
                        "UnicodeDecodeError",
                        "'ascii' codec can't decode byte: ordinal not in range(128)".to_string(),
                    ));
                }
            }
            Ok(out)
        }
        "latin-1" | "iso-8859-1" | "8859" | "cp819" | "latin1" | "l1" => {
            Ok(raw.iter().map(|&b| b as char).collect())
        }
        _ => Ok(String::from_utf8_lossy(raw).into_owned()),
    }
}

fn open_native_temp_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 7 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 7 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let path = require_str(fn_name, &args[0].value, "path")?;
    let mode = require_str(fn_name, &args[1].value, "mode")?;
    let encoding_opt = match args[2].value.kind() {
        ValueKind::None => None,
        ValueKind::Str(s) => Some(s.to_string()),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "open() argument 'encoding' must be str or None, not {}",
                    value_type_name_str(&args[2].value),
                ),
            ));
        }
    };
    let errors_opt = match args[3].value.kind() {
        ValueKind::None => None,
        ValueKind::Str(s) => Some(s.to_string()),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "open() argument 'errors' must be str or None, not {}",
                    value_type_name_str(&args[3].value),
                ),
            ));
        }
    };
    let newline_opt = match args[4].value.kind() {
        ValueKind::None => None,
        ValueKind::Str(s) => Some(s.to_string()),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "open() argument 'newline' must be str or None, not {}",
                    value_type_name_str(&args[4].value),
                ),
            ));
        }
    };
    let unlink_immediately = match args[5].value.kind() {
        ValueKind::Bool(b) => b,
        ValueKind::Int(n) => n != 0,
        ValueKind::None => false,
        _ => true,
    };
    let display_name = args[6].value.clone();

    // Validate mode characters.
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
        if newline_opt.is_some() {
            return Err(PyError::named(
                "ValueError",
                "binary mode doesn't take a newline argument".to_string(),
            ));
        }
        (None, None, None)
    } else {
        let (norm, disp) = match encoding_opt {
            Some(enc) => {
                let norm = validate_encoding(&enc)?;
                (Some(norm), Some(enc))
            }
            None => (Some("utf-8".to_string()), Some("utf-8".to_string())),
        };
        let errs = Some(errors_opt.unwrap_or_else(|| "strict".to_string()));
        (norm, disp, errs)
    };

    let file = create_exclusive_file(&path)?;
    if unlink_immediately && let Err(err) = std::fs::remove_file(&path) {
        drop(file);
        return Err(PyError::from_io_error(&err, Some(&path)));
    }

    let fd = NEXT_TEMP_FILENO.fetch_add(1, Ordering::Relaxed);
    let final_display_name = if unlink_immediately {
        Value::int(fd)
    } else {
        display_name
    };

    let is_readable = mode.contains('r') || has_plus;
    let is_writable = mode.contains('w') || mode.contains('a') || mode.contains('x') || has_plus;
    let is_append = mode.contains('a');

    let state = TempFileState {
        file: Some(file),
        fd,
        path,
        display_name: final_display_name,
        mode,
        is_binary,
        is_readable,
        is_writable,
        is_append,
        encoding: encoding_norm,
        encoding_display,
        errors: errors_str,
        newline: newline_opt,
        closed: false,
    };
    Ok(Value::builtin_object(TEMP_FILE_OPS, Box::new(state)))
}

fn temp_file_has_method(name: &str) -> bool {
    matches!(
        name,
        "__enter__"
            | "__exit__"
            | "__iter__"
            | "__next__"
            | "close"
            | "flush"
            | "fileno"
            | "isatty"
            | "readable"
            | "writable"
            | "seekable"
            | "read"
            | "read1"
            | "readinto"
            | "readline"
            | "readlines"
            | "seek"
            | "tell"
            | "truncate"
            | "write"
            | "writelines"
    )
}

impl BuiltinTypeOps for TempFileOps {
    fn type_name(&self) -> &'static str {
        TEMP_FILE_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        if let Some(s) = borrow.downcast_ref::<TempFileState>() {
            if s.is_binary {
                format!("<_io.BufferedRandom name={}>", s.display_name.repr_raw())
            } else {
                let enc = s.encoding_display.as_deref().unwrap_or("utf-8");
                format!(
                    "<_io.TextIOWrapper name={} mode='{}' encoding='{}'>",
                    s.display_name.repr_raw(),
                    s.mode,
                    enc
                )
            }
        } else {
            "<temp file object>".to_string()
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn is_iterable(&self) -> bool {
        true
    }

    fn is_iterator(&self) -> bool {
        true
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        temp_readline_opt(state, None)
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<TempFileState>()?;
        match name {
            "name" => Some(s.display_name.clone()),
            "mode" => Some(Value::string(s.mode.clone())),
            "closed" => Some(Value::bool_(s.closed)),
            "encoding" => s
                .encoding_display
                .as_ref()
                .map(|enc| Value::string(enc.clone())),
            "errors" => s.errors.as_ref().map(|e| Value::string(e.clone())),
            "newlines" => {
                if s.is_binary {
                    None
                } else {
                    Some(Value::none())
                }
            }
            _ => None,
        }
    }

    fn has_method(&self, name: &str) -> bool {
        temp_file_has_method(name)
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        temp_call_method(state, name, &args)
    }
}

fn temp_call_method(state: &BuiltinState, method: &str, args: &[Value]) -> Result<Value> {
    match method {
        "__enter__" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<TempFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::builtin_object_shared(
                TEMP_FILE_OPS,
                Rc::clone(state),
            ))
        }
        "__exit__" => {
            temp_close(state)?;
            Ok(Value::none())
        }
        "__iter__" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<TempFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::builtin_object_shared(
                TEMP_FILE_OPS,
                Rc::clone(state),
            ))
        }
        "__next__" => match temp_readline_opt(state, None)? {
            Some(line) => Ok(line),
            None => Err(PyError::named("StopIteration", String::new())),
        },
        "close" => {
            temp_close(state)?;
            Ok(Value::none())
        }
        "flush" => {
            let mut borrow = state.borrow_mut();
            let s = borrow
                .downcast_mut::<TempFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
            let path = s.path.clone();
            let file = s.file.as_mut().ok_or_else(closed_file_error)?;
            file.flush()
                .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
            Ok(Value::none())
        }
        "fileno" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<TempFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::int(s.fd))
        }
        "isatty" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<TempFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(false))
        }
        "readable" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<TempFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(s.is_readable))
        }
        "writable" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<TempFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(s.is_writable))
        }
        "seekable" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<TempFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(true))
        }
        "tell" => {
            let mut borrow = state.borrow_mut();
            let s = borrow
                .downcast_mut::<TempFileState>()
                .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
            let path = s.path.clone();
            let file = s.file.as_mut().ok_or_else(closed_file_error)?;
            let pos = file
                .stream_position()
                .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
            Ok(Value::int(pos as i64))
        }
        "seek" => temp_seek(state, args),
        "truncate" => temp_truncate(state, args),
        "read" | "read1" => temp_read(state, args),
        "readinto" => temp_readinto(state, args),
        "readline" => temp_readline(state, args),
        "readlines" => temp_readlines(state, args),
        "write" => temp_write(state, args),
        "writelines" => temp_writelines(state, args),
        _ => Err(PyError::named(
            "AttributeError",
            format!("'{TEMP_FILE_TYPE_NAME}' object has no attribute '{method}'"),
        )),
    }
}

fn temp_close(state: &BuiltinState) -> Result<()> {
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<TempFileState>()
        .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
    if !s.closed {
        if let Some(mut file) = s.file.take() {
            let _ = file.flush();
            drop(file);
        }
        s.closed = true;
    }
    Ok(())
}

fn temp_seek(state: &BuiltinState, args: &[Value]) -> Result<Value> {
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
        .downcast_mut::<TempFileState>()
        .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
    let path = s.path.clone();
    let is_binary = s.is_binary;
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
        1 => {
            if !is_binary && offset != 0 {
                return Err(PyError::named(
                    "io.UnsupportedOperation",
                    "can't do nonzero cur-relative seeks".to_string(),
                ));
            }
            SeekFrom::Current(offset)
        }
        2 => {
            if !is_binary && offset != 0 {
                return Err(PyError::named(
                    "io.UnsupportedOperation",
                    "can't do nonzero end-relative seeks".to_string(),
                ));
            }
            SeekFrom::End(offset)
        }
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

fn temp_truncate(state: &BuiltinState, args: &[Value]) -> Result<Value> {
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
        .downcast_mut::<TempFileState>()
        .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
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

fn temp_read(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let limit = match args.first().map(|v| v.kind()) {
        None | Some(ValueKind::None) => None,
        Some(ValueKind::Int(n)) if n < 0 => None,
        Some(ValueKind::Int(n)) => Some(n as usize),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "read() argument must be int or None".to_string(),
            ));
        }
    };
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<TempFileState>()
        .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
    if s.closed {
        return Err(closed_file_error());
    }
    if !s.is_readable {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not readable".to_string(),
        ));
    }
    let path = s.path.clone();
    let is_binary = s.is_binary;
    let enc = s.encoding.clone().unwrap_or_else(|| "utf-8".to_string());
    let errs = s.errors.clone().unwrap_or_else(|| "strict".to_string());
    let newline = s.newline.clone();
    let file = s.file.as_mut().ok_or_else(closed_file_error)?;

    if is_binary {
        let mut buf = Vec::new();
        match limit {
            Some(n) => {
                buf.resize(n, 0);
                let mut total = 0;
                while total < n {
                    let read = file
                        .read(&mut buf[total..])
                        .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
                    if read == 0 {
                        break;
                    }
                    total += read;
                }
                buf.truncate(total);
            }
            None => {
                file.read_to_end(&mut buf)
                    .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
            }
        }
        return Ok(Value::bytes(buf));
    }

    // Text mode read: decode from current file position; if a character limit
    // is given, advance the file cursor by the exact byte length consumed.
    let start_pos = file
        .stream_position()
        .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)
        .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
    let decoded = decode_text(&raw, &enc, &errs)?;
    let translate_newlines = newline.is_none();
    let normalized = if translate_newlines {
        decoded.replace("\r\n", "\n")
    } else {
        decoded
    };
    if let Some(want_chars) = limit {
        let taken: String = normalized.chars().take(want_chars).collect();
        if taken.len() < normalized.len() {
            // Rewind to the exact byte offset after the consumed portion.
            let raw_prefix = if translate_newlines {
                // Count raw bytes corresponding to `want_chars` logical chars.
                let mut chars_seen = 0usize;
                let mut byte_idx = 0usize;
                let raw_decoded = decode_text(&raw, &enc, &errs)?;
                let mut iter = raw_decoded.char_indices().peekable();
                while let Some((idx, ch)) = iter.next() {
                    if chars_seen >= want_chars {
                        byte_idx = idx;
                        break;
                    }
                    if ch == '\r' && iter.peek().is_some_and(|&(_, next)| next == '\n') {
                        iter.next();
                    }
                    chars_seen += 1;
                    byte_idx = iter.peek().map_or(raw_decoded.len(), |&(ni, _)| ni);
                }
                encode_text(&raw_decoded[..byte_idx], &enc, &errs)?.len()
            } else {
                encode_text(&taken, &enc, &errs)?.len()
            };
            file.seek(SeekFrom::Start(start_pos + raw_prefix as u64))
                .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        }
        return Ok(Value::string(taken));
    }
    Ok(Value::string(normalized))
}

fn temp_readinto(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let target = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "readinto() takes exactly one argument (0 given)".to_string(),
        )
    })?;
    let Some(buf_rc) = pyrust_builtins::bytearray::as_bytearray_rc(target) else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "readinto() argument must be read-write bytes-like object, not {}",
                value_type_name_str(target)
            ),
        ));
    };
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<TempFileState>()
        .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
    if s.closed {
        return Err(closed_file_error());
    }
    if !s.is_readable {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not readable".to_string(),
        ));
    }
    if !s.is_binary {
        return Err(PyError::named(
            "AttributeError",
            "'_io.TextIOWrapper' object has no attribute 'readinto'".to_string(),
        ));
    }
    let path = s.path.clone();
    let file = s.file.as_mut().ok_or_else(closed_file_error)?;
    let mut buf = buf_rc.borrow_mut();
    let n = file
        .read(&mut buf[..])
        .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
    Ok(Value::int(n as i64))
}

fn temp_readline_opt(state: &BuiltinState, limit: Option<usize>) -> Result<Option<Value>> {
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<TempFileState>()
        .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
    if s.closed {
        return Err(closed_file_error());
    }
    if !s.is_readable {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not readable".to_string(),
        ));
    }
    let path = s.path.clone();
    let is_binary = s.is_binary;
    let enc = s.encoding.clone().unwrap_or_else(|| "utf-8".to_string());
    let errs = s.errors.clone().unwrap_or_else(|| "strict".to_string());
    let newline = s.newline.clone();
    let file = s.file.as_mut().ok_or_else(closed_file_error)?;

    if limit == Some(0) {
        return Ok(Some(if is_binary {
            Value::bytes(Vec::new())
        } else {
            Value::string(String::new())
        }));
    }

    let mut line_bytes = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if is_binary && limit.is_some_and(|n| line_bytes.len() >= n) {
            break;
        }
        let read = file
            .read(&mut byte)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        if read == 0 {
            break;
        }
        line_bytes.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    if line_bytes.is_empty() {
        return Ok(None);
    }
    if is_binary {
        Ok(Some(Value::bytes(line_bytes)))
    } else {
        let mut decoded = decode_text(&line_bytes, &enc, &errs)?;
        if newline.is_none() {
            decoded = decoded.replace("\r\n", "\n");
        }
        if let Some(lim) = limit {
            let trimmed: String = decoded.chars().take(lim).collect();
            if trimmed.len() < decoded.len() {
                let consumed_raw = encode_text(&trimmed, &enc, &errs)?.len();
                let rewind = line_bytes.len() as i64 - consumed_raw as i64;
                if rewind > 0 {
                    let _ = file.seek(SeekFrom::Current(-rewind));
                }
                decoded = trimmed;
            }
        }
        Ok(Some(Value::string(decoded)))
    }
}

fn temp_readline(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let limit = match args.first().map(|v| v.kind()) {
        None | Some(ValueKind::None) => None,
        Some(ValueKind::Int(n)) if n < 0 => None,
        Some(ValueKind::Int(n)) => Some(n as usize),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "readline() argument must be int or None".to_string(),
            ));
        }
    };
    match temp_readline_opt(state, limit)? {
        Some(line) => Ok(line),
        None => {
            let borrow = state.borrow();
            let is_binary = borrow
                .downcast_ref::<TempFileState>()
                .is_some_and(|s| s.is_binary);
            Ok(if is_binary {
                Value::bytes(Vec::new())
            } else {
                Value::string(String::new())
            })
        }
    }
}

fn temp_readlines(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let hint = match args.first().map(|v| v.kind()) {
        None | Some(ValueKind::None) => None,
        Some(ValueKind::Int(n)) if n <= 0 => None,
        Some(ValueKind::Int(n)) => Some(n as usize),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "readlines() argument must be int or None".to_string(),
            ));
        }
    };
    let mut lines = Vec::new();
    let mut total_size = 0usize;
    while let Some(line) = temp_readline_opt(state, None)? {
        total_size += match line.kind() {
            ValueKind::Bytes(b) => b.len(),
            ValueKind::Str(s) => s.len(),
            _ => 0,
        };
        lines.push(line);
        if hint.is_some_and(|h| total_size >= h) {
            break;
        }
    }
    Ok(Value::list(lines))
}

fn temp_write(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let arg = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "write() takes exactly one argument (0 given)".to_string(),
        )
    })?;
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<TempFileState>()
        .ok_or_else(|| PyError::runtime("internal: bad temp file state".to_string()))?;
    if s.closed {
        return Err(closed_file_error());
    }
    if !s.is_writable {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not writable".to_string(),
        ));
    }
    let path = s.path.clone();
    let is_binary = s.is_binary;
    let is_append = s.is_append;
    let enc = s.encoding.clone().unwrap_or_else(|| "utf-8".to_string());
    let errs = s.errors.clone().unwrap_or_else(|| "strict".to_string());
    let newline = s.newline.clone();
    let file = s.file.as_mut().ok_or_else(closed_file_error)?;

    if is_append {
        file.seek(SeekFrom::End(0))
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
    }

    if is_binary {
        let data = match arg.kind() {
            ValueKind::Bytes(b) => b.as_ref().clone(),
            _ => {
                if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(arg) {
                    snap
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "a bytes-like object is required, not '{}'",
                            value_type_name_str(arg)
                        ),
                    ));
                }
            }
        };
        let len = data.len() as i64;
        file.write_all(&data)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        Ok(Value::int(len))
    } else {
        let text = match arg.kind() {
            ValueKind::Str(st) => st.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "write() argument must be str, not {}",
                        value_type_name_str(arg)
                    ),
                ));
            }
        };
        let char_len = text.chars().count() as i64;
        let to_encode = match newline.as_deref() {
            None => {
                #[cfg(windows)]
                {
                    text.replace('\n', "\r\n")
                }
                #[cfg(not(windows))]
                {
                    text
                }
            }
            Some("") | Some("\n") => text,
            Some(nl) => text.replace('\n', nl),
        };
        let encoded = encode_text(&to_encode, &enc, &errs)?;
        file.write_all(&encoded)
            .map_err(|e| PyError::from_io_error(&e, Some(&path)))?;
        Ok(Value::int(char_len))
    }
}

fn temp_writelines(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let items = match args.first().map(|v| v.kind()) {
        Some(ValueKind::List(list)) => list.clone(),
        Some(ValueKind::Tuple(tup)) => tup.to_vec(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "writelines() argument must be a sequence".to_string(),
            ));
        }
    };
    for item in items {
        temp_write(state, &[item])?;
    }
    Ok(Value::none())
}

pyrust_module! {
    fn _mkstemp_open(args) -> Result<Value> {
        mkstemp_open_impl(FN_NAME, args)
    }

    fn _open_native_temp(args) -> Result<Value> {
        open_native_temp_impl(FN_NAME, args)
    }
}
