// `subprocess` module — body for the `subprocess @inject` entry in
// `pyrust_builtin_modules!` (issue #3187).
//
// Native Rust helpers back process spawning (`std::process::Command`),
// streaming stdin/stdout/stderr pipes (`PipeWriterOps` / `PipeReaderOps`),
// merged `stderr=STDOUT` via `std::io::pipe()`, deadlock-free concurrent
// `communicate()` reader/writer threads, non-blocking `poll()`, bounded
// `wait(timeout=...)`, and `terminate()` / `kill()` / `send_signal()`.
//
// The public Python 3.12 API (`PIPE`, `STDOUT`, `DEVNULL`, `SubprocessError`,
// `CalledProcessError`, `TimeoutExpired`, `CompletedProcess`, `Popen`, `run`,
// `call`, `check_call`, `check_output`, `list2cmdline`, `getstatusoutput`,
// `getoutput`) lives in `subprocess_py.py` and is injected by
// `inject_python_members`.
//
// Reference: <https://docs.python.org/3/library/subprocess.html>

use std::any::Any;
use std::cell::RefCell;
use std::io::{Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps};
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, reject_keyword_args_expanded, value_type_name_str,
};
use crate::value::{PyKey, Value, ValueKind};

/// Python-source definitions for the `subprocess` module.
const SUBPROCESS_PY_SOURCE: &str = include_str!("subprocess_py.py");

/// Private native helpers moved into the exec namespace and hidden from
/// `dir(subprocess)` so the module surface matches CPython 3.12.
const SUBPROCESS_NATIVE_HELPERS: [&str; 1] = ["_spawn_process"];

/// Public attributes exported onto the `subprocess` module (platform-specific
/// names are only defined in `subprocess_py.py` on Windows and therefore
/// automatically skipped on POSIX).
const SUBPROCESS_PY_EXPORTS: &[&str] = &[
    "__all__",
    "PIPE",
    "STDOUT",
    "DEVNULL",
    "SubprocessError",
    "CalledProcessError",
    "TimeoutExpired",
    "CompletedProcess",
    "Popen",
    "run",
    "call",
    "check_call",
    "check_output",
    "list2cmdline",
    "getstatusoutput",
    "getoutput",
    "STARTUPINFO",
    "CREATE_NEW_CONSOLE",
    "CREATE_NEW_PROCESS_GROUP",
    "STD_INPUT_HANDLE",
    "STD_OUTPUT_HANDLE",
    "STD_ERROR_HANDLE",
    "SW_HIDE",
    "STARTF_USESTDHANDLES",
    "STARTF_USESHOWWINDOW",
    "ABOVE_NORMAL_PRIORITY_CLASS",
    "BELOW_NORMAL_PRIORITY_CLASS",
    "HIGH_PRIORITY_CLASS",
    "IDLE_PRIORITY_CLASS",
    "NORMAL_PRIORITY_CLASS",
    "REALTIME_PRIORITY_CLASS",
    "CREATE_NO_WINDOW",
    "DETACHED_PROCESS",
    "CREATE_DEFAULT_ERROR_MODE",
    "CREATE_BREAKAWAY_FROM_JOB",
];

const PIPE_SENTINEL: i64 = -1;
const STDOUT_SENTINEL: i64 = -2;
const DEVNULL_SENTINEL: i64 = -3;

/// Execute `SUBPROCESS_PY_SOURCE` once, wire its wrappers to `_spawn_process`,
/// hide private helper names from `dir(subprocess)`, and export the public
/// CPython 3.12 `subprocess` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in SUBPROCESS_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source_with_filename(
        SUBPROCESS_PY_SOURCE,
        "<frozen subprocess>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("subprocess: exec namespace not a dict".into()))?;
    for &name in SUBPROCESS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("subprocess"));
                class.error_name = match name {
                    "SubprocessError" => Some("subprocess.SubprocessError"),
                    "CalledProcessError" => Some("subprocess.CalledProcessError"),
                    "TimeoutExpired" => Some("subprocess.TimeoutExpired"),
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

// ── Text / codec helpers ─────────────────────────────────────────────────────

fn closed_file_error() -> PyError {
    PyError::named("ValueError", "I/O operation on closed file.".to_string())
}

fn normalise_encoding(enc: &str) -> String {
    enc.to_ascii_lowercase().replace('_', "-")
}

fn validate_encoding_and_errors(enc: &str, errors: &str) -> Result<(String, String)> {
    let norm = normalise_encoding(enc);
    let effective = if norm == "locale" {
        "utf-8".to_string()
    } else {
        norm
    };
    // Probe with empty bytes so unknown encodings raise LookupError up front.
    let _ = pyrust_builtins::bytes::decode_bytes(b"", &effective, errors)?;
    Ok((effective, errors.to_string()))
}

fn decode_bytes_to_string(raw: &[u8], enc: &str, errors: &str) -> Result<String> {
    let val = pyrust_builtins::bytes::decode_bytes(raw, enc, errors)?;
    match val.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        _ => Ok(String::new()),
    }
}

fn encode_string_to_bytes(text: &str, enc: &str, errors: &str) -> Result<Vec<u8>> {
    let val = pyrust_builtins::string::encode_str_to_bytes(text, enc, errors)?;
    match val.kind() {
        ValueKind::Bytes(b) => Ok(b.as_slice().to_vec()),
        _ => Ok(Vec::new()),
    }
}

/// Translate universal newlines (`\r\n` and lone `\r`) into `\n`, tracking a
/// trailing `\r` across chunk boundaries via `pending_cr`.
fn normalize_universal_newlines(chunk: &str, pending_cr: &mut bool) -> String {
    let mut out = String::with_capacity(chunk.len());
    let mut chars = chunk.chars().peekable();
    if *pending_cr {
        *pending_cr = false;
        if chars.peek() == Some(&'\n') {
            chars.next();
        }
    }
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            out.push('\n');
            if chars.peek() == Some(&'\n') {
                chars.next();
            } else if chars.peek().is_none() {
                *pending_cr = true;
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn io_error_to_py(err: &std::io::Error) -> PyError {
    if err.kind() == std::io::ErrorKind::BrokenPipe {
        let raw = err.raw_os_error().unwrap_or(32);
        return PyError::named("BrokenPipeError", format!("[Errno {raw}] Broken pipe"));
    }
    PyError::from_io_error(err, None)
}

// ── Pipe read stream (`Popen.stdout` / `Popen.stderr`) ───────────────────────

enum ReadPipeHandle {
    Stdout(ChildStdout),
    Stderr(ChildStderr),
    Pipe(std::io::PipeReader),
}

impl Read for ReadPipeHandle {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Stdout(s) => s.read(buf),
            Self::Stderr(s) => s.read(buf),
            Self::Pipe(p) => p.read(buf),
        }
    }
}

struct PipeReaderState {
    handle: Option<ReadPipeHandle>,
    raw_buf: Vec<u8>,
    raw_pos: usize,
    text_buf: String,
    text_pos: usize,
    pending_cr: bool,
    eof: bool,
    is_binary: bool,
    encoding: String,
    encoding_display: String,
    errors: String,
    closed: bool,
}

impl PipeReaderState {
    fn new(
        handle: ReadPipeHandle,
        is_binary: bool,
        encoding: String,
        encoding_display: String,
        errors: String,
    ) -> Self {
        Self {
            handle: Some(handle),
            raw_buf: Vec::new(),
            raw_pos: 0,
            text_buf: String::new(),
            text_pos: 0,
            pending_cr: false,
            eof: false,
            is_binary,
            encoding,
            encoding_display,
            errors,
            closed: false,
        }
    }

    fn take_for_communicate(&mut self) -> (Vec<u8>, Option<ReadPipeHandle>) {
        let remaining = if self.raw_pos < self.raw_buf.len() {
            self.raw_buf[self.raw_pos..].to_vec()
        } else {
            Vec::new()
        };
        self.raw_buf.clear();
        self.raw_pos = 0;
        let handle = self.handle.take();
        (remaining, handle)
    }

    fn fill_raw_chunk(&mut self) -> Result<usize> {
        if self.eof {
            return Ok(0);
        }
        if self.raw_pos > 0 && self.raw_pos >= self.raw_buf.len() {
            self.raw_buf.clear();
            self.raw_pos = 0;
        }
        let Some(ref mut handle) = self.handle else {
            self.eof = true;
            return Ok(0);
        };
        let mut tmp = [0u8; 8192];
        loop {
            match handle.read(&mut tmp) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(0);
                }
                Ok(n) => {
                    self.raw_buf.extend_from_slice(&tmp[..n]);
                    return Ok(n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_error_to_py(&e)),
            }
        }
    }

    fn read_bytes_all(&mut self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        if self.raw_pos < self.raw_buf.len() {
            out.extend_from_slice(&self.raw_buf[self.raw_pos..]);
            self.raw_buf.clear();
            self.raw_pos = 0;
        }
        if let Some(ref mut handle) = self.handle {
            handle
                .read_to_end(&mut out)
                .map_err(|e| io_error_to_py(&e))?;
        }
        self.eof = true;
        Ok(out)
    }

    fn read_bytes_limit(&mut self, limit: usize) -> Result<Vec<u8>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(limit.min(8192));
        while out.len() < limit {
            let avail = self.raw_buf.len().saturating_sub(self.raw_pos);
            if avail > 0 {
                let take = avail.min(limit - out.len());
                out.extend_from_slice(&self.raw_buf[self.raw_pos..self.raw_pos + take]);
                self.raw_pos += take;
                if self.raw_pos >= self.raw_buf.len() {
                    self.raw_buf.clear();
                    self.raw_pos = 0;
                }
                continue;
            }
            if self.fill_raw_chunk()? == 0 {
                break;
            }
        }
        Ok(out)
    }

    fn readline_bytes(&mut self, limit: Option<usize>) -> Result<Vec<u8>> {
        if limit == Some(0) {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        loop {
            let slice = &self.raw_buf[self.raw_pos..];
            if !slice.is_empty() {
                let remaining_limit = limit.map(|l| l.saturating_sub(out.len()));
                let search_len = match remaining_limit {
                    Some(rem) => slice.len().min(rem),
                    None => slice.len(),
                };
                if let Some(nl_idx) = slice[..search_len].iter().position(|&b| b == b'\n') {
                    let take = nl_idx + 1;
                    out.extend_from_slice(&slice[..take]);
                    self.raw_pos += take;
                    if self.raw_pos >= self.raw_buf.len() {
                        self.raw_buf.clear();
                        self.raw_pos = 0;
                    }
                    return Ok(out);
                }
                out.extend_from_slice(&slice[..search_len]);
                self.raw_pos += search_len;
                if self.raw_pos >= self.raw_buf.len() {
                    self.raw_buf.clear();
                    self.raw_pos = 0;
                }
                if limit.is_some_and(|l| out.len() >= l) {
                    return Ok(out);
                }
            }
            if self.fill_raw_chunk()? == 0 {
                return Ok(out);
            }
        }
    }

    /// Decode available raw bytes into `self.text_buf`, leaving any trailing
    /// incomplete UTF-8 sequence in `self.raw_buf` until more bytes or EOF
    /// arrive.
    fn decode_raw_into_text(&mut self) -> Result<()> {
        if self.raw_pos >= self.raw_buf.len() {
            return Ok(());
        }
        let is_utf8 = matches!(self.encoding.as_str(), "utf-8" | "utf8" | "u8" | "utf");
        let slice = &self.raw_buf[self.raw_pos..];
        let decode_len = if is_utf8 && !self.eof {
            match std::str::from_utf8(slice) {
                Ok(_) => slice.len(),
                Err(err) => {
                    if err.error_len().is_none() {
                        // Incomplete multi-byte sequence at end of buffer; wait for more bytes.
                        err.valid_up_to()
                    } else {
                        // Genuine invalid byte sequence — let codec error handler process it.
                        slice.len()
                    }
                }
            }
        } else {
            slice.len()
        };
        if decode_len == 0 {
            return Ok(());
        }
        let decoded = decode_bytes_to_string(&slice[..decode_len], &self.encoding, &self.errors)?;
        self.raw_pos += decode_len;
        if self.raw_pos >= self.raw_buf.len() {
            self.raw_buf.clear();
            self.raw_pos = 0;
        }
        let normalized = normalize_universal_newlines(&decoded, &mut self.pending_cr);
        self.text_buf.push_str(&normalized);
        Ok(())
    }

    fn compact_text_buf(&mut self) {
        if self.text_pos >= self.text_buf.len() {
            self.text_buf.clear();
            self.text_pos = 0;
        } else if self.text_pos > 4096 {
            self.text_buf.drain(..self.text_pos);
            self.text_pos = 0;
        }
    }

    fn read_text_all(&mut self) -> Result<String> {
        let raw = self.read_bytes_all()?;
        if !raw.is_empty() {
            let decoded = decode_bytes_to_string(&raw, &self.encoding, &self.errors)?;
            let normalized = normalize_universal_newlines(&decoded, &mut self.pending_cr);
            self.text_buf.push_str(&normalized);
        }
        let out = self.text_buf[self.text_pos..].to_string();
        self.text_buf.clear();
        self.text_pos = 0;
        self.pending_cr = false;
        Ok(out)
    }

    fn read_text_limit(&mut self, limit: usize) -> Result<String> {
        if limit == 0 {
            return Ok(String::new());
        }
        let mut out = String::new();
        let mut count = 0usize;
        while count < limit {
            self.decode_raw_into_text()?;
            if self.text_pos < self.text_buf.len() {
                let tail = &self.text_buf[self.text_pos..];
                let mut byte_adv = 0usize;
                for ch in tail.chars() {
                    if count >= limit {
                        break;
                    }
                    out.push(ch);
                    byte_adv += ch.len_utf8();
                    count += 1;
                }
                self.text_pos += byte_adv;
                self.compact_text_buf();
                if count >= limit {
                    break;
                }
            }
            if self.fill_raw_chunk()? == 0 {
                self.decode_raw_into_text()?;
                if self.text_pos < self.text_buf.len() {
                    let tail = &self.text_buf[self.text_pos..];
                    let mut byte_adv = 0usize;
                    for ch in tail.chars() {
                        if count >= limit {
                            break;
                        }
                        out.push(ch);
                        byte_adv += ch.len_utf8();
                        count += 1;
                    }
                    self.text_pos += byte_adv;
                    self.compact_text_buf();
                }
                break;
            }
        }
        Ok(out)
    }

    fn readline_text(&mut self, limit: Option<usize>) -> Result<String> {
        if limit == Some(0) {
            return Ok(String::new());
        }
        let mut out = String::new();
        let mut count = 0usize;
        loop {
            self.decode_raw_into_text()?;
            if self.text_pos < self.text_buf.len() {
                let tail = &self.text_buf[self.text_pos..];
                let mut byte_adv = 0usize;
                let mut hit_nl = false;
                for ch in tail.chars() {
                    if limit.is_some_and(|l| count >= l) {
                        break;
                    }
                    out.push(ch);
                    byte_adv += ch.len_utf8();
                    count += 1;
                    if ch == '\n' {
                        hit_nl = true;
                        break;
                    }
                }
                self.text_pos += byte_adv;
                self.compact_text_buf();
                if hit_nl || limit.is_some_and(|l| count >= l) {
                    return Ok(out);
                }
            }
            if self.fill_raw_chunk()? == 0 {
                self.decode_raw_into_text()?;
                if self.text_pos < self.text_buf.len() {
                    let tail = &self.text_buf[self.text_pos..];
                    let mut byte_adv = 0usize;
                    for ch in tail.chars() {
                        if limit.is_some_and(|l| count >= l) {
                            break;
                        }
                        out.push(ch);
                        byte_adv += ch.len_utf8();
                        count += 1;
                        if ch == '\n' {
                            break;
                        }
                    }
                    self.text_pos += byte_adv;
                    self.compact_text_buf();
                }
                return Ok(out);
            }
        }
    }
}

struct PipeReaderOps;
const PIPE_READER_OPS: &PipeReaderOps = &PipeReaderOps;
const PIPE_READER_TYPE_NAME: &str = "_io.BufferedReader";

fn pipe_reader_has_method(name: &str) -> bool {
    matches!(
        name,
        "__enter__"
            | "__exit__"
            | "__iter__"
            | "__next__"
            | "close"
            | "flush"
            | "isatty"
            | "readable"
            | "writable"
            | "seekable"
            | "read"
            | "read1"
            | "readline"
            | "readlines"
    )
}

fn parse_optional_size(fn_name: &str, args: &[Value]) -> Result<Option<usize>> {
    match args.first().map(|v| v.kind()) {
        None | Some(ValueKind::None) => Ok(None),
        Some(ValueKind::Int(n)) if n < 0 => Ok(None),
        Some(ValueKind::Int(n)) => Ok(Some(n as usize)),
        Some(ValueKind::Bool(b)) => Ok(Some(usize::from(b))),
        Some(_) => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() argument must be int or None"),
        )),
    }
}

fn pipe_reader_readline_opt(state: &BuiltinState, limit: Option<usize>) -> Result<Option<Value>> {
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<PipeReaderState>()
        .ok_or_else(|| PyError::runtime("internal: bad pipe reader state".into()))?;
    if s.closed {
        return Err(closed_file_error());
    }
    if s.is_binary {
        let line = s.readline_bytes(limit)?;
        if line.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Value::bytes(line)))
        }
    } else {
        let line = s.readline_text(limit)?;
        if line.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Value::string(line)))
        }
    }
}

impl BuiltinTypeOps for PipeReaderOps {
    fn type_name(&self) -> &'static str {
        PIPE_READER_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        if let Some(s) = borrow.downcast_ref::<PipeReaderState>() {
            if s.is_binary {
                "<_io.BufferedReader name='<pipe>'>".to_string()
            } else {
                format!(
                    "<_io.TextIOWrapper name='<pipe>' mode='r' encoding='{}'>",
                    s.encoding_display
                )
            }
        } else {
            "<pipe reader>".to_string()
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
        pipe_reader_readline_opt(state, None)
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<PipeReaderState>()?;
        match name {
            "closed" => Some(Value::bool_(s.closed)),
            "mode" => Some(Value::string(if s.is_binary { "rb" } else { "r" })),
            "encoding" => {
                if s.is_binary {
                    None
                } else {
                    Some(Value::string(s.encoding_display.clone()))
                }
            }
            "errors" => {
                if s.is_binary {
                    None
                } else {
                    Some(Value::string(s.errors.clone()))
                }
            }
            _ => None,
        }
    }

    fn has_method(&self, name: &str) -> bool {
        pipe_reader_has_method(name)
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        match name {
            "__enter__" | "__iter__" => {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<PipeReaderState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe reader state".into()))?;
                if s.closed {
                    return Err(closed_file_error());
                }
                Ok(Value::builtin_object_shared(
                    PIPE_READER_OPS,
                    Rc::clone(state),
                ))
            }
            "__exit__" | "close" => {
                let mut borrow = state.borrow_mut();
                let s = borrow
                    .downcast_mut::<PipeReaderState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe reader state".into()))?;
                s.handle = None;
                s.closed = true;
                Ok(Value::none())
            }
            "__next__" => match pipe_reader_readline_opt(state, None)? {
                Some(line) => Ok(line),
                None => Err(PyError::named("StopIteration", String::new())),
            },
            "flush" => {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<PipeReaderState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe reader state".into()))?;
                if s.closed {
                    return Err(closed_file_error());
                }
                Ok(Value::none())
            }
            "readable" => {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<PipeReaderState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe reader state".into()))?;
                if s.closed {
                    return Err(closed_file_error());
                }
                Ok(Value::bool_(true))
            }
            "writable" | "seekable" | "isatty" => {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<PipeReaderState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe reader state".into()))?;
                if s.closed {
                    return Err(closed_file_error());
                }
                Ok(Value::bool_(false))
            }
            "read" | "read1" => {
                let limit = parse_optional_size(name, &args)?;
                let mut borrow = state.borrow_mut();
                let s = borrow
                    .downcast_mut::<PipeReaderState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe reader state".into()))?;
                if s.closed {
                    return Err(closed_file_error());
                }
                if s.is_binary {
                    let bytes = match limit {
                        None => s.read_bytes_all()?,
                        Some(n) => s.read_bytes_limit(n)?,
                    };
                    Ok(Value::bytes(bytes))
                } else {
                    let text = match limit {
                        None => s.read_text_all()?,
                        Some(n) => s.read_text_limit(n)?,
                    };
                    Ok(Value::string(text))
                }
            }
            "readline" => {
                let limit = parse_optional_size("readline", &args)?;
                let is_binary = {
                    let borrow = state.borrow();
                    let s = borrow.downcast_ref::<PipeReaderState>().ok_or_else(|| {
                        PyError::runtime("internal: bad pipe reader state".into())
                    })?;
                    if s.closed {
                        return Err(closed_file_error());
                    }
                    s.is_binary
                };
                match pipe_reader_readline_opt(state, limit)? {
                    Some(v) => Ok(v),
                    None => {
                        if is_binary {
                            Ok(Value::bytes(Vec::new()))
                        } else {
                            Ok(Value::string(String::new()))
                        }
                    }
                }
            }
            "readlines" => {
                let hint = parse_optional_size("readlines", &args)?;
                let mut lines = Vec::new();
                let mut total = 0usize;
                while let Some(line) = pipe_reader_readline_opt(state, None)? {
                    let len = match line.kind() {
                        ValueKind::Bytes(b) => b.len(),
                        ValueKind::Str(s) => s.chars().count(),
                        _ => 0,
                    };
                    lines.push(line);
                    total += len;
                    if let Some(h) = hint
                        && h > 0
                        && total >= h
                    {
                        break;
                    }
                }
                Ok(Value::list(lines))
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("'{PIPE_READER_TYPE_NAME}' object has no attribute '{name}'"),
            )),
        }
    }
}

// ── Pipe write stream (`Popen.stdin`) ────────────────────────────────────────

struct PipeWriterState {
    handle: Option<ChildStdin>,
    is_binary: bool,
    encoding: String,
    encoding_display: String,
    errors: String,
    closed: bool,
}

struct PipeWriterOps;
const PIPE_WRITER_OPS: &PipeWriterOps = &PipeWriterOps;
const PIPE_WRITER_TYPE_NAME: &str = "_io.BufferedWriter";

fn pipe_writer_has_method(name: &str) -> bool {
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
            | "write"
            | "writelines"
    )
}

fn pipe_writer_write_one(s: &mut PipeWriterState, val: &Value) -> Result<i64> {
    if s.closed {
        return Err(closed_file_error());
    }
    let is_binary = s.is_binary;
    let enc = s.encoding.clone();
    let errors = s.errors.clone();
    let handle = s.handle.as_mut().ok_or_else(closed_file_error)?;

    if is_binary {
        let data: Vec<u8> = match val.kind() {
            ValueKind::Bytes(b) => b.as_slice().to_vec(),
            _ => {
                if let Some(ba) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
                    ba
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "a bytes-like object is required, not '{}'",
                            value_type_name_str(val),
                        ),
                    ));
                }
            }
        };
        handle.write_all(&data).map_err(|e| io_error_to_py(&e))?;
        Ok(data.len() as i64)
    } else {
        let text = match val.kind() {
            ValueKind::Str(text) => text.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "write() argument must be str, not {}",
                        value_type_name_str(val),
                    ),
                ));
            }
        };
        let char_len = text.chars().count() as i64;
        #[cfg(windows)]
        let wire_text = text.replace('\n', "\r\n");
        #[cfg(not(windows))]
        let wire_text = text;
        let encoded = encode_string_to_bytes(&wire_text, &enc, &errors)?;
        handle.write_all(&encoded).map_err(|e| io_error_to_py(&e))?;
        Ok(char_len)
    }
}

impl BuiltinTypeOps for PipeWriterOps {
    fn type_name(&self) -> &'static str {
        PIPE_WRITER_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        if let Some(s) = borrow.downcast_ref::<PipeWriterState>() {
            if s.is_binary {
                "<_io.BufferedWriter name='<pipe>'>".to_string()
            } else {
                format!(
                    "<_io.TextIOWrapper name='<pipe>' mode='w' encoding='{}'>",
                    s.encoding_display
                )
            }
        } else {
            "<pipe writer>".to_string()
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<PipeWriterState>()?;
        match name {
            "closed" => Some(Value::bool_(s.closed)),
            "mode" => Some(Value::string(if s.is_binary { "wb" } else { "w" })),
            "encoding" => {
                if s.is_binary {
                    None
                } else {
                    Some(Value::string(s.encoding_display.clone()))
                }
            }
            "errors" => {
                if s.is_binary {
                    None
                } else {
                    Some(Value::string(s.errors.clone()))
                }
            }
            _ => None,
        }
    }

    fn has_method(&self, name: &str) -> bool {
        pipe_writer_has_method(name)
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        match name {
            "__enter__" => {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<PipeWriterState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe writer state".into()))?;
                if s.closed {
                    return Err(closed_file_error());
                }
                Ok(Value::builtin_object_shared(
                    PIPE_WRITER_OPS,
                    Rc::clone(state),
                ))
            }
            "__exit__" | "close" => {
                let mut borrow = state.borrow_mut();
                let s = borrow
                    .downcast_mut::<PipeWriterState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe writer state".into()))?;
                if !s.closed {
                    if let Some(mut handle) = s.handle.take() {
                        let _ = handle.flush();
                        drop(handle);
                    }
                    s.closed = true;
                }
                Ok(Value::none())
            }
            "flush" => {
                let mut borrow = state.borrow_mut();
                let s = borrow
                    .downcast_mut::<PipeWriterState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe writer state".into()))?;
                let handle = s.handle.as_mut().ok_or_else(closed_file_error)?;
                handle.flush().map_err(|e| io_error_to_py(&e))?;
                Ok(Value::none())
            }
            "writable" => {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<PipeWriterState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe writer state".into()))?;
                if s.closed {
                    return Err(closed_file_error());
                }
                Ok(Value::bool_(true))
            }
            "readable" | "seekable" | "isatty" => {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<PipeWriterState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe writer state".into()))?;
                if s.closed {
                    return Err(closed_file_error());
                }
                Ok(Value::bool_(false))
            }
            "write" => {
                if args.len() != 1 {
                    return Err(PyError::named(
                        "TypeError",
                        format!("write() takes exactly one argument ({} given)", args.len()),
                    ));
                }
                let mut borrow = state.borrow_mut();
                let s = borrow
                    .downcast_mut::<PipeWriterState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe writer state".into()))?;
                let written = pipe_writer_write_one(s, &args[0])?;
                Ok(Value::int(written))
            }
            "writelines" => {
                if args.len() != 1 {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "writelines() takes exactly one argument ({} given)",
                            args.len()
                        ),
                    ));
                }
                let items: Vec<Value> = match args[0].kind() {
                    ValueKind::List(l) => l.to_vec(),
                    ValueKind::Tuple(t) => t.to_vec(),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!("'{}' object is not iterable", value_type_name_str(&args[0])),
                        ));
                    }
                };
                let mut borrow = state.borrow_mut();
                let s = borrow
                    .downcast_mut::<PipeWriterState>()
                    .ok_or_else(|| PyError::runtime("internal: bad pipe writer state".into()))?;
                for item in &items {
                    pipe_writer_write_one(s, item)?;
                }
                Ok(Value::none())
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("'{PIPE_WRITER_TYPE_NAME}' object has no attribute '{name}'"),
            )),
        }
    }
}

// ── Background drain tasks for deadlock-free `communicate()` ─────────────────

struct DrainTask {
    buf: Arc<Mutex<Vec<u8>>>,
    done_rx: mpsc::Receiver<()>,
    join: Option<JoinHandle<()>>,
    finished: bool,
}

impl DrainTask {
    fn spawn(initial: Vec<u8>, mut handle: ReadPipeHandle) -> Self {
        let buf = Arc::new(Mutex::new(initial));
        let buf_clone = Arc::clone(&buf);
        let (done_tx, done_rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut tmp = [0u8; 8192];
            loop {
                match handle.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut guard) = buf_clone.lock() {
                            guard.extend_from_slice(&tmp[..n]);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = done_tx.send(());
        });
        Self {
            buf,
            done_rx,
            join: Some(join),
            finished: false,
        }
    }

    fn wait_until(&mut self, deadline: Option<Instant>) -> bool {
        if self.finished {
            return true;
        }
        let ok = match deadline {
            None => {
                let _ = self.done_rx.recv();
                true
            }
            Some(dl) => {
                let now = Instant::now();
                if now >= dl {
                    self.done_rx.try_recv().is_ok()
                } else {
                    match self.done_rx.recv_timeout(dl - now) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
                        Err(mpsc::RecvTimeoutError::Timeout) => false,
                    }
                }
            }
        };
        if ok {
            self.finished = true;
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
        }
        ok
    }

    fn snapshot(&self) -> Vec<u8> {
        self.buf.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

// ── Native process handle (`Popen._handle`) ──────────────────────────────────

struct CommunicateResult {
    timed_out: bool,
    stdout: Option<Vec<u8>>,
    stderr: Option<Vec<u8>>,
    returncode: Option<i64>,
}

struct SubprocessHandleState {
    child: Child,
    pid: u32,
    returncode: Option<i64>,
    stdin_state: Option<BuiltinState>,
    stdout_state: Option<BuiltinState>,
    stderr_state: Option<BuiltinState>,
    stdin_val: Value,
    stdout_val: Value,
    stderr_val: Value,
    stdin_writer: Option<JoinHandle<()>>,
    stdout_drain: Option<DrainTask>,
    stderr_drain: Option<DrainTask>,
}

fn exit_status_to_returncode(status: ExitStatus) -> i64 {
    if let Some(code) = status.code() {
        return code as i64;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return -(sig as i64);
        }
    }
    0
}

impl SubprocessHandleState {
    fn poll_inner(&mut self) -> Result<Option<i64>> {
        if let Some(rc) = self.returncode {
            return Ok(Some(rc));
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                let rc = exit_status_to_returncode(status);
                self.returncode = Some(rc);
                Ok(Some(rc))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(PyError::from_io_error(&e, None)),
        }
    }

    fn wait_inner(&mut self, timeout: Option<f64>) -> Result<Option<i64>> {
        if let Some(rc) = self.returncode {
            return Ok(Some(rc));
        }
        match timeout {
            None => {
                let status = self
                    .child
                    .wait()
                    .map_err(|e| PyError::from_io_error(&e, None))?;
                let rc = exit_status_to_returncode(status);
                self.returncode = Some(rc);
                Ok(Some(rc))
            }
            Some(secs) => {
                let dur = if secs <= 0.0 || secs.is_nan() {
                    Duration::ZERO
                } else if secs.is_infinite() {
                    return self.wait_inner(None);
                } else {
                    Duration::from_secs_f64(secs)
                };
                let deadline = Instant::now() + dur;
                let mut sleep_dur = Duration::from_micros(500);
                loop {
                    if let Some(rc) = self.poll_inner()? {
                        return Ok(Some(rc));
                    }
                    let now = Instant::now();
                    if now >= deadline {
                        return Ok(None);
                    }
                    let rem = deadline - now;
                    thread::sleep(sleep_dur.min(rem));
                    sleep_dur = (sleep_dur * 2).min(Duration::from_millis(10));
                }
            }
        }
    }

    fn terminate_inner(&mut self) -> Result<()> {
        if self.poll_inner()?.is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            // SAFETY: `self.pid` is a live child pid not yet reaped.
            let rc = unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGTERM) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    return Err(PyError::from_io_error(&err, None));
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            self.kill_inner()
        }
    }

    fn kill_inner(&mut self) -> Result<()> {
        if self.poll_inner()?.is_some() {
            return Ok(());
        }
        match self.child.kill() {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // On Windows, ERROR_ACCESS_DENIED can occur if the child already exited.
                if self.poll_inner()?.is_some() {
                    Ok(())
                } else {
                    Err(PyError::from_io_error(&e, None))
                }
            }
            Err(e) => Err(PyError::from_io_error(&e, None)),
        }
    }

    fn send_signal_inner(&mut self, sig: i64) -> Result<()> {
        if self.poll_inner()?.is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            // SAFETY: `self.pid` is a live child pid not yet reaped.
            let rc = unsafe { libc::kill(self.pid as libc::pid_t, sig as libc::c_int) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::ESRCH) {
                    return Err(PyError::from_io_error(&err, None));
                }
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            // CPython on Windows maps SIGTERM (15) to terminate() and rejects
            // other signal numbers with ValueError.
            if sig == 15 {
                self.terminate_inner()
            } else {
                Err(PyError::named(
                    "ValueError",
                    format!("Unsupported signal: {sig}"),
                ))
            }
        }
    }

    fn communicate_inner(
        &mut self,
        input_bytes: Option<Vec<u8>>,
        timeout: Option<f64>,
    ) -> Result<CommunicateResult> {
        let deadline = timeout.and_then(|secs| {
            if secs.is_infinite() && secs.is_sign_positive() {
                None
            } else if secs <= 0.0 || secs.is_nan() {
                Some(Instant::now())
            } else {
                Some(Instant::now() + Duration::from_secs_f64(secs))
            }
        });

        // Validate that user didn't close stdout/stderr before communicate().
        if self.stdout_drain.is_none()
            && let Some(ref st) = self.stdout_state
            && st
                .borrow()
                .downcast_ref::<PipeReaderState>()
                .is_some_and(|r| r.closed)
        {
            return Err(PyError::named(
                "ValueError",
                "read of closed file".to_string(),
            ));
        }
        if self.stderr_drain.is_none()
            && let Some(ref st) = self.stderr_state
            && st
                .borrow()
                .downcast_ref::<PipeReaderState>()
                .is_some_and(|r| r.closed)
        {
            return Err(PyError::named(
                "ValueError",
                "read of closed file".to_string(),
            ));
        }

        // Start stdout/stderr drain threads if not already running.
        if self.stdout_drain.is_none()
            && let Some(ref st) = self.stdout_state
        {
            let (initial, handle_opt) = {
                let mut b = st.borrow_mut();
                let r = b
                    .downcast_mut::<PipeReaderState>()
                    .ok_or_else(|| PyError::runtime("internal: bad stdout state".into()))?;
                r.take_for_communicate()
            };
            if let Some(handle) = handle_opt {
                self.stdout_drain = Some(DrainTask::spawn(initial, handle));
            }
        }
        if self.stderr_drain.is_none()
            && let Some(ref st) = self.stderr_state
        {
            let (initial, handle_opt) = {
                let mut b = st.borrow_mut();
                let r = b
                    .downcast_mut::<PipeReaderState>()
                    .ok_or_else(|| PyError::runtime("internal: bad stderr state".into()))?;
                r.take_for_communicate()
            };
            if let Some(handle) = handle_opt {
                self.stderr_drain = Some(DrainTask::spawn(initial, handle));
            }
        }

        // Feed stdin on a dedicated writer thread so large input + large output
        // never deadlocks on OS pipe buffers.
        if let Some(ref st) = self.stdin_state {
            let mut b = st.borrow_mut();
            let w = b
                .downcast_mut::<PipeWriterState>()
                .ok_or_else(|| PyError::runtime("internal: bad stdin state".into()))?;
            if w.closed {
                if input_bytes.is_some() {
                    return Err(closed_file_error());
                }
            } else {
                let stdin_opt = w.handle.take();
                w.closed = true;
                if let Some(mut stdin_pipe) = stdin_opt {
                    if let Some(data) = input_bytes
                        && !data.is_empty()
                    {
                        self.stdin_writer = Some(thread::spawn(move || {
                            let _ = stdin_pipe.write_all(&data);
                            let _ = stdin_pipe.flush();
                            drop(stdin_pipe);
                        }));
                    } else {
                        drop(stdin_pipe);
                    }
                }
            }
        }

        // Wait for child process and drain threads respecting `deadline`.
        let mut timed_out = false;
        let remaining_secs = |dl: Option<Instant>| -> Option<f64> {
            dl.map(|d| d.saturating_duration_since(Instant::now()).as_secs_f64())
        };

        if self.wait_inner(remaining_secs(deadline))?.is_none() {
            timed_out = true;
        }
        if !timed_out
            && let Some(ref mut drain) = self.stdout_drain
            && !drain.wait_until(deadline)
        {
            timed_out = true;
        }
        if !timed_out
            && let Some(ref mut drain) = self.stderr_drain
            && !drain.wait_until(deadline)
        {
            timed_out = true;
        }

        if timed_out {
            let out_snap = self.stdout_drain.as_ref().map(DrainTask::snapshot);
            let err_snap = self.stderr_drain.as_ref().map(DrainTask::snapshot);
            return Ok(CommunicateResult {
                timed_out: true,
                stdout: out_snap,
                stderr: err_snap,
                returncode: None,
            });
        }

        // Completed cleanly: join writer thread, harvest drain buffers, and
        // mark stream wrappers closed.
        if let Some(j) = self.stdin_writer.take() {
            let _ = j.join();
        }
        let out_bytes = if let Some(drain) = self.stdout_drain.take() {
            Some(drain.snapshot())
        } else if self.stdout_state.is_some() {
            Some(Vec::new())
        } else {
            None
        };
        let err_bytes = if let Some(drain) = self.stderr_drain.take() {
            Some(drain.snapshot())
        } else if self.stderr_state.is_some() {
            Some(Vec::new())
        } else {
            None
        };
        if let Some(ref st) = self.stdout_state
            && let Some(r) = st.borrow_mut().downcast_mut::<PipeReaderState>()
        {
            r.closed = true;
        }
        if let Some(ref st) = self.stderr_state
            && let Some(r) = st.borrow_mut().downcast_mut::<PipeReaderState>()
        {
            r.closed = true;
        }
        Ok(CommunicateResult {
            timed_out: false,
            stdout: out_bytes,
            stderr: err_bytes,
            returncode: self.returncode,
        })
    }
}

struct SubprocessHandleOps;
const SUBPROCESS_HANDLE_OPS: &SubprocessHandleOps = &SubprocessHandleOps;
const SUBPROCESS_HANDLE_TYPE_NAME: &str = "_subprocess.Handle";

impl BuiltinTypeOps for SubprocessHandleOps {
    fn type_name(&self) -> &'static str {
        SUBPROCESS_HANDLE_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        if let Some(h) = borrow.downcast_ref::<SubprocessHandleState>() {
            format!("<_subprocess.Handle pid={}>", h.pid)
        } else {
            "<_subprocess.Handle>".to_string()
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let h = borrow.downcast_ref::<SubprocessHandleState>()?;
        match name {
            "pid" => Some(Value::int(h.pid as i64)),
            "stdin" => Some(h.stdin_val.clone()),
            "stdout" => Some(h.stdout_val.clone()),
            "stderr" => Some(h.stderr_val.clone()),
            _ => None,
        }
    }

    fn has_method(&self, name: &str) -> bool {
        matches!(
            name,
            "poll" | "wait" | "communicate" | "terminate" | "kill" | "send_signal"
        )
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let mut borrow = state.borrow_mut();
        let h = borrow
            .downcast_mut::<SubprocessHandleState>()
            .ok_or_else(|| PyError::runtime("internal: bad subprocess handle".into()))?;
        match name {
            "poll" => match h.poll_inner()? {
                Some(rc) => Ok(Value::int(rc)),
                None => Ok(Value::none()),
            },
            "wait" => {
                let timeout = parse_optional_timeout(args.first())?;
                match h.wait_inner(timeout)? {
                    Some(rc) => Ok(Value::int(rc)),
                    None => Ok(Value::none()),
                }
            }
            "communicate" => {
                let input_bytes = match args.first().map(|v| v.kind()) {
                    None | Some(ValueKind::None) => None,
                    Some(ValueKind::Bytes(b)) => Some(b.as_slice().to_vec()),
                    Some(_) => {
                        if let Some(ba) =
                            pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0])
                        {
                            Some(ba)
                        } else {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "a bytes-like object is required, not '{}'",
                                    value_type_name_str(&args[0]),
                                ),
                            ));
                        }
                    }
                };
                let timeout = parse_optional_timeout(args.get(1))?;
                let res = h.communicate_inner(input_bytes, timeout)?;
                let out_val = match res.stdout {
                    Some(b) => Value::bytes(b),
                    None => Value::none(),
                };
                let err_val = match res.stderr {
                    Some(b) => Value::bytes(b),
                    None => Value::none(),
                };
                let rc_val = match res.returncode {
                    Some(rc) => Value::int(rc),
                    None => Value::none(),
                };
                Ok(Value::tuple(vec![
                    Value::bool_(res.timed_out),
                    out_val,
                    err_val,
                    rc_val,
                ]))
            }
            "terminate" => {
                h.terminate_inner()?;
                Ok(Value::none())
            }
            "kill" => {
                h.kill_inner()?;
                Ok(Value::none())
            }
            "send_signal" => {
                let sig = match args.first().map(|v| v.kind()) {
                    Some(ValueKind::Int(n)) => n,
                    Some(ValueKind::Bool(b)) => i64::from(b),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "send_signal() argument must be int".to_string(),
                        ));
                    }
                };
                h.send_signal_inner(sig)?;
                Ok(Value::none())
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("'{SUBPROCESS_HANDLE_TYPE_NAME}' object has no attribute '{name}'"),
            )),
        }
    }
}

fn parse_optional_timeout(val: Option<&Value>) -> Result<Option<f64>> {
    match val.map(|v| v.kind()) {
        None | Some(ValueKind::None) => Ok(None),
        Some(ValueKind::Int(n)) => Ok(Some(n as f64)),
        Some(ValueKind::Float(f)) => Ok(Some(f)),
        Some(ValueKind::Bool(b)) => Ok(Some(if b { 1.0 } else { 0.0 })),
        Some(_) => Err(PyError::named(
            "TypeError",
            format!(
                "timeout must be a real number or None, not {}",
                value_type_name_str(val.unwrap()),
            ),
        )),
    }
}

// ── Stdio configuration & `_spawn_process` ───────────────────────────────────

fn dup_stdout_stdio() -> Stdio {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsFd;
        if let Ok(owned) = std::io::stdout().as_fd().try_clone_to_owned() {
            return Stdio::from(owned);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        if let Ok(owned) = std::io::stdout().as_handle().try_clone_to_owned() {
            return Stdio::from(owned);
        }
    }
    Stdio::inherit()
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

fn optional_str(fn_name: &str, value: &Value, what: &str) -> Result<Option<String>> {
    match value.kind() {
        ValueKind::None => Ok(None),
        ValueKind::Str(s) => Ok(Some(s.to_string())),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() {what} must be str or None, not {}",
                value_type_name_str(value),
            ),
        )),
    }
}

fn optional_int(fn_name: &str, value: &Value, what: &str) -> Result<Option<i64>> {
    match value.kind() {
        ValueKind::None => Ok(None),
        ValueKind::Int(n) => Ok(Some(n)),
        ValueKind::Bool(b) => Ok(Some(i64::from(b))),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() {what} must be int or None, not {}",
                value_type_name_str(value),
            ),
        )),
    }
}

fn check_no_nul(s: &str) -> Result<()> {
    if s.contains('\0') {
        return Err(PyError::named(
            "ValueError",
            "embedded null byte".to_string(),
        ));
    }
    Ok(())
}

fn stdio_from_spec(spec: Option<i64>, allow_stdout_sentinel: bool) -> Result<Stdio> {
    match spec {
        None => Ok(Stdio::inherit()),
        Some(PIPE_SENTINEL) => Ok(Stdio::piped()),
        Some(DEVNULL_SENTINEL) => Ok(Stdio::null()),
        Some(STDOUT_SENTINEL) if allow_stdout_sentinel => Ok(Stdio::inherit()),
        Some(fd) if fd >= 0 => {
            let file = super::os::clone_fd(fd).map_err(|e| PyError::from_io_error(&e, None))?;
            Ok(Stdio::from(file))
        }
        Some(other) => Err(PyError::named(
            "ValueError",
            format!("invalid stdio specification: {other}"),
        )),
    }
}

fn spawn_process_impl(fn_name: &str, args: &[ExpandedCallArg]) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 12 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 12 arguments ({} given)",
                args.len()
            ),
        ));
    }

    let program = require_str(fn_name, &args[0].value, "program")?;
    check_no_nul(&program)?;

    let argv: Vec<String> = match args[1].value.kind() {
        ValueKind::List(list) => {
            let mut out = Vec::with_capacity(list.len());
            for item in list.iter() {
                let s = require_str(fn_name, item, "argv element")?;
                check_no_nul(&s)?;
                out.push(s);
            }
            out
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}() argv must be list, not {}",
                    value_type_name_str(&args[1].value),
                ),
            ));
        }
    };

    let arg0 = optional_str(fn_name, &args[2].value, "arg0")?;
    if let Some(ref a0) = arg0 {
        check_no_nul(a0)?;
    }

    let raw_args = optional_str(fn_name, &args[3].value, "raw_args")?;
    if let Some(ref r) = raw_args {
        check_no_nul(r)?;
    }

    let cwd = optional_str(fn_name, &args[4].value, "cwd")?;
    if let Some(ref dir) = cwd {
        check_no_nul(dir)?;
    }

    let env_pairs: Option<Vec<(String, String)>> = match args[5].value.kind() {
        ValueKind::None => None,
        ValueKind::List(list) => {
            let mut pairs = Vec::with_capacity(list.len());
            for item in list.iter() {
                match item.kind() {
                    ValueKind::Tuple(t) if t.len() == 2 => {
                        let k = require_str(fn_name, &t[0], "env key")?;
                        let v = require_str(fn_name, &t[1], "env value")?;
                        check_no_nul(&k)?;
                        check_no_nul(&v)?;
                        if k.contains('=') {
                            return Err(PyError::named(
                                "ValueError",
                                "illegal environment variable name".to_string(),
                            ));
                        }
                        pairs.push((k, v));
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "env items must be (str, str) pairs".to_string(),
                        ));
                    }
                }
            }
            Some(pairs)
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}() env must be list or None, not {}",
                    value_type_name_str(&args[5].value),
                ),
            ));
        }
    };

    let stdin_spec = optional_int(fn_name, &args[6].value, "stdin")?;
    let stdout_spec = optional_int(fn_name, &args[7].value, "stdout")?;
    let stderr_spec = optional_int(fn_name, &args[8].value, "stderr")?;

    let text_mode = match args[9].value.kind() {
        ValueKind::Bool(b) => b,
        ValueKind::Int(n) => n != 0,
        ValueKind::None => false,
        _ => true,
    };
    let encoding_opt = optional_str(fn_name, &args[10].value, "encoding")?;
    let errors_opt = optional_str(fn_name, &args[11].value, "errors")?;

    let (encoding_norm, encoding_display, errors_str) = if text_mode {
        let enc_display = encoding_opt.unwrap_or_else(|| "utf-8".to_string());
        let errs = errors_opt.unwrap_or_else(|| "strict".to_string());
        let (norm, errs) = validate_encoding_and_errors(&enc_display, &errs)?;
        let shown = if normalise_encoding(&enc_display) == "locale" {
            "utf-8".to_string()
        } else {
            enc_display
        };
        (norm, shown, errs)
    } else {
        (
            "utf-8".to_string(),
            "utf-8".to_string(),
            "strict".to_string(),
        )
    };

    let mut cmd = Command::new(&program);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let _ = &raw_args;
        if let Some(ref a0) = arg0 {
            cmd.arg0(a0);
        }
        cmd.args(&argv);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = &arg0;
        if let Some(ref raw) = raw_args {
            if !raw.is_empty() {
                cmd.raw_arg(raw);
            }
        } else {
            cmd.args(&argv);
        }
    }

    if let Some(ref dir) = cwd {
        cmd.current_dir(dir);
    }

    if let Some(ref pairs) = env_pairs {
        cmd.env_clear();
        for (k, v) in pairs {
            cmd.env(k, v);
        }
    }

    cmd.stdin(stdio_from_spec(stdin_spec, false)?);

    let mut merged_stdout_reader: Option<ReadPipeHandle> = None;
    if stderr_spec == Some(STDOUT_SENTINEL) {
        match stdout_spec {
            Some(PIPE_SENTINEL) => {
                let (reader, writer) =
                    std::io::pipe().map_err(|e| PyError::from_io_error(&e, None))?;
                let writer2 = writer
                    .try_clone()
                    .map_err(|e| PyError::from_io_error(&e, None))?;
                cmd.stdout(Stdio::from(writer));
                cmd.stderr(Stdio::from(writer2));
                merged_stdout_reader = Some(ReadPipeHandle::Pipe(reader));
            }
            Some(DEVNULL_SENTINEL) => {
                cmd.stdout(Stdio::null());
                cmd.stderr(Stdio::null());
            }
            Some(fd) if fd >= 0 => {
                let f1 = super::os::clone_fd(fd).map_err(|e| PyError::from_io_error(&e, None))?;
                let f2 = f1
                    .try_clone()
                    .map_err(|e| PyError::from_io_error(&e, None))?;
                cmd.stdout(Stdio::from(f1));
                cmd.stderr(Stdio::from(f2));
            }
            None => {
                cmd.stdout(Stdio::inherit());
                cmd.stderr(dup_stdout_stdio());
            }
            Some(other) => {
                return Err(PyError::named(
                    "ValueError",
                    format!("invalid stdout specification: {other}"),
                ));
            }
        }
    } else {
        cmd.stdout(stdio_from_spec(stdout_spec, false)?);
        cmd.stderr(stdio_from_spec(stderr_spec, false)?);
    }

    let mut child = cmd.spawn().map_err(|e| {
        let filename = if let Some(ref dir) = cwd
            && !std::path::Path::new(dir).exists()
        {
            dir.as_str()
        } else {
            program.as_str()
        };
        PyError::from_io_error(&e, Some(filename))
    })?;
    let pid = child.id();
    let is_binary = !text_mode;

    let (stdin_state, stdin_val) = match child.stdin.take() {
        Some(handle) => {
            let st: BuiltinState = Rc::new(RefCell::new(Box::new(PipeWriterState {
                handle: Some(handle),
                is_binary,
                encoding: encoding_norm.clone(),
                encoding_display: encoding_display.clone(),
                errors: errors_str.clone(),
                closed: false,
            }) as Box<dyn Any>));
            let val = Value::builtin_object_shared(PIPE_WRITER_OPS, Rc::clone(&st));
            (Some(st), val)
        }
        None => (None, Value::none()),
    };

    let stdout_handle =
        merged_stdout_reader.or_else(|| child.stdout.take().map(ReadPipeHandle::Stdout));
    let (stdout_state, stdout_val) = match stdout_handle {
        Some(handle) => {
            let st: BuiltinState = Rc::new(RefCell::new(Box::new(PipeReaderState::new(
                handle,
                is_binary,
                encoding_norm.clone(),
                encoding_display.clone(),
                errors_str.clone(),
            )) as Box<dyn Any>));
            let val = Value::builtin_object_shared(PIPE_READER_OPS, Rc::clone(&st));
            (Some(st), val)
        }
        None => (None, Value::none()),
    };

    let (stderr_state, stderr_val) = match child.stderr.take() {
        Some(handle) => {
            let st: BuiltinState = Rc::new(RefCell::new(Box::new(PipeReaderState::new(
                ReadPipeHandle::Stderr(handle),
                is_binary,
                encoding_norm,
                encoding_display,
                errors_str,
            )) as Box<dyn Any>));
            let val = Value::builtin_object_shared(PIPE_READER_OPS, Rc::clone(&st));
            (Some(st), val)
        }
        None => (None, Value::none()),
    };

    let handle_state = SubprocessHandleState {
        child,
        pid,
        returncode: None,
        stdin_state,
        stdout_state,
        stderr_state,
        stdin_val,
        stdout_val,
        stderr_val,
        stdin_writer: None,
        stdout_drain: None,
        stderr_drain: None,
    };

    Ok(Value::builtin_object(
        SUBPROCESS_HANDLE_OPS,
        Box::new(handle_state),
    ))
}

pyrust_module! {
    fn _spawn_process(args) -> Result<Value> {
        spawn_process_impl(FN_NAME, args)
    }
}
