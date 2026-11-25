//! `open()` and file I/O — relocated from `pyrust-core` Tier 1.
//!
//! Built-in file objects are not a Tier 1 type: they have no literal syntax
//! and no compile-time-specializable payload (just a `Box<dyn Any>` already).
//! All state and operations live here; the VM sees only a generic
//! `BuiltinObject` whose `BuiltinTypeOps` impl dispatches through this module.

use std::any::Any;
use std::io::{Read, Seek, SeekFrom, Write};
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps, PyError, Result, Value, ValueKind};

use crate::bytes::decode_bytes;
use crate::string::encode_str_to_bytes;

mod stdio;

pub use stdio::{
    STDIO_OPS, STDIO_TYPE_NAME, StdioKind, StdioOps, StdioState, default_stdio_kind, make_stderr,
    make_stdin, make_stdout, stdio_has_method,
};

/// Options passed to [`open_with_options`] and [`open_fd`].
#[derive(Debug, Clone, Copy)]
pub struct FileOpenOptions<'a> {
    pub mode: &'a str,
    pub buffering: i64,
    pub encoding: Option<&'a str>,
    pub errors: Option<&'a str>,
    pub newline: Option<&'a str>,
    pub closefd: bool,
}

#[derive(Debug, Clone)]
pub enum FileName {
    Path(String),
    Fd(i64),
}

impl FileName {
    fn display_for_error(&self) -> Option<&str> {
        match self {
            FileName::Path(p) => Some(p.as_str()),
            FileName::Fd(_) => None,
        }
    }

    fn repr_fragment(&self) -> String {
        match self {
            FileName::Path(p) => format!("'{p}'"),
            FileName::Fd(fd) => fd.to_string(),
        }
    }

    fn as_value(&self) -> Value {
        match self {
            FileName::Path(p) => Value::string(p.clone()),
            FileName::Fd(fd) => Value::int(*fd),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewlineMode {
    /// `newline=None`: universal newlines on read (translating `\r\n` and `\r`
    /// to `\n`); on write `\n` translates to `os.linesep`.
    UniversalTranslate,
    /// `newline=""`: universal newlines on read without translation; on write
    /// no translation.
    UniversalNoTranslate,
    /// `newline="\n"`: split only on `\n`, no translation on read or write.
    Lf,
    /// `newline="\r"`: split only on `\r`, no translation on read; on write
    /// `\n` translates to `\r`.
    Cr,
    /// `newline="\r\n"`: split only on `\r\n`, no translation on read; on write
    /// `\n` translates to `\r\n`.
    CrLf,
}

#[derive(Debug, Default, Clone, Copy)]
struct ObservedNewlines {
    seen_cr: bool,
    seen_lf: bool,
    seen_crlf: bool,
}

impl ObservedNewlines {
    fn as_value(&self) -> Value {
        let mut items: Vec<&'static str> = Vec::new();
        if self.seen_cr {
            items.push("\r");
        }
        if self.seen_lf {
            items.push("\n");
        }
        if self.seen_crlf {
            items.push("\r\n");
        }
        match items.len() {
            0 => Value::none(),
            1 => Value::string(items[0]),
            _ => Value::tuple(items.into_iter().map(Value::string).collect()),
        }
    }
}

pub struct FileState {
    pub name: FileName,
    pub mode: String,
    pub closed: bool,
    /// Underlying OS file handle (None once closed).
    pub os_file: Option<std::fs::File>,
    /// Optional integer fd & close callback for `open(fd, closefd=True/False)`.
    pub fd_num: Option<i64>,
    pub closefd: bool,
    pub close_fd_fn: Option<fn(i64) -> std::io::Result<()>>,
    /// Buffering policy (`-1` default, `0` unbuffered binary, `1` line-buffered
    /// text, `>1` explicit buffer size).
    pub buffering: i64,
    /// For text read modes: lazily loaded decoded text buffer.
    pub content: String,
    /// For binary buffered read modes: lazily loaded raw byte buffer.
    pub content_bytes: Vec<u8>,
    /// Whether `content` / `content_bytes` has been loaded from `os_file`.
    pub read_loaded: bool,
    /// OS file byte offset corresponding to `pos == 0` in `content` / `content_bytes`.
    pub read_base_offset: u64,
    /// Byte offset inside `content` or `content_bytes`.
    pub pos: usize,
    /// Pending write bytes waiting for `flush()` / `close()`.
    pub write_buf_bytes: Vec<u8>,
    /// Whether a BOM has already been emitted for encodings like `utf-8-sig`.
    pub bom_written: bool,
    pub is_readable_mode: bool,
    pub is_writable_mode: bool,
    pub is_append: bool,
    /// Whether the file was opened in binary mode (`'b'` in mode string).
    pub is_binary: bool,
    /// Normalised encoding name for codec routing (`None` for binary mode).
    pub encoding: Option<String>,
    /// Original user-supplied encoding string preserved verbatim for `.encoding` and `repr()`.
    pub encoding_display: Option<String>,
    /// Error handler name for text mode (`"strict"` by default).
    pub errors: String,
    /// Newline translation mode for text mode.
    pub newline: NewlineMode,
    /// Observed line endings while reading in universal-newline mode.
    observed_newlines: ObservedNewlines,
}

impl FileState {
    fn is_readable(&self) -> bool {
        self.is_readable_mode
    }

    fn is_writable(&self) -> bool {
        self.is_writable_mode
    }

    fn error_path(&self) -> Option<&str> {
        self.name.display_for_error()
    }
}

pub struct FileOps;

pub const FILE_OPS: &FileOps = &FileOps;
pub const TYPE_NAME: &str = "_io.TextIOWrapper";

impl BuiltinTypeOps for FileOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        if let Some(s) = borrow.downcast_ref::<FileState>() {
            let name_frag = s.name.repr_fragment();
            if s.is_binary {
                if s.buffering == 0 {
                    let closefd_str = if s.closefd { "True" } else { "False" };
                    format!(
                        "<_io.FileIO name={name_frag} mode='{}' closefd={closefd_str}>",
                        s.mode
                    )
                } else if s.is_readable_mode && s.is_writable_mode {
                    format!("<_io.BufferedRandom name={name_frag}>")
                } else if s.is_writable_mode {
                    format!("<_io.BufferedWriter name={name_frag}>")
                } else {
                    format!("<_io.BufferedReader name={name_frag}>")
                }
            } else {
                let enc = s.encoding_display.as_deref().unwrap_or("UTF-8");
                format!(
                    "<_io.TextIOWrapper name={name_frag} mode='{}' encoding='{enc}'>",
                    s.mode
                )
            }
        } else {
            "<file object>".to_string()
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        read_line_value(state, None)
    }

    fn is_iterable(&self) -> bool {
        true
    }

    fn is_iterator(&self) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<FileState>()?;
        match name {
            "name" => Some(s.name.as_value()),
            "mode" => Some(Value::string(s.mode.clone())),
            "closed" => Some(Value::bool_(s.closed)),
            "closefd" => Some(Value::bool_(s.closefd)),
            "encoding" => {
                if s.is_binary {
                    None
                } else {
                    let enc = s.encoding_display.as_deref().unwrap_or("UTF-8");
                    Some(Value::string(enc))
                }
            }
            "errors" => {
                if s.is_binary {
                    None
                } else {
                    Some(Value::string(s.errors.clone()))
                }
            }
            "line_buffering" => {
                if s.is_binary {
                    None
                } else {
                    Some(Value::bool_(s.buffering == 1))
                }
            }
            "newlines" => {
                if s.is_binary {
                    None
                } else {
                    Some(s.observed_newlines.as_value())
                }
            }
            _ => None,
        }
    }

    fn has_method(&self, name: &str) -> bool {
        has_method(name)
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        call_file_method_inner(state, name, &args)
    }
}

/// Normalise an encoding name to the canonical lowercase form used internally.
fn normalise_encoding(enc: &str) -> String {
    enc.to_ascii_lowercase().replace('_', "-")
}

struct ParsedMode {
    is_read: bool,
    is_write: bool,
    is_append: bool,
    is_create: bool,
    is_plus: bool,
    is_binary: bool,
}

fn parse_mode(mode: &str) -> Result<ParsedMode> {
    let mut is_read = false;
    let mut is_write = false;
    let mut is_append = false;
    let mut is_create = false;
    let mut is_plus = false;
    let mut is_binary = false;
    let mut is_text = false;

    for c in mode.chars() {
        let slot = match c {
            'r' => &mut is_read,
            'w' => &mut is_write,
            'a' => &mut is_append,
            'x' => &mut is_create,
            '+' => &mut is_plus,
            'b' => &mut is_binary,
            't' => &mut is_text,
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    format!("invalid mode: '{mode}'"),
                ));
            }
        };
        if *slot {
            return Err(PyError::named(
                "ValueError",
                format!("invalid mode: '{mode}'"),
            ));
        }
        *slot = true;
    }

    if is_binary && is_text {
        return Err(PyError::named(
            "ValueError",
            "can't have text and binary mode at once".to_string(),
        ));
    }

    let primary_count = usize::from(is_read)
        + usize::from(is_write)
        + usize::from(is_append)
        + usize::from(is_create);
    if primary_count != 1 {
        return Err(PyError::named(
            "ValueError",
            "must have exactly one of create/read/write/append mode".to_string(),
        ));
    }

    Ok(ParsedMode {
        is_read,
        is_write,
        is_append,
        is_create,
        is_plus,
        is_binary,
    })
}

fn parse_newline(newline: Option<&str>) -> Result<NewlineMode> {
    match newline {
        None => Ok(NewlineMode::UniversalTranslate),
        Some("") => Ok(NewlineMode::UniversalNoTranslate),
        Some("\n") => Ok(NewlineMode::Lf),
        Some("\r") => Ok(NewlineMode::Cr),
        Some("\r\n") => Ok(NewlineMode::CrLf),
        Some(other) => Err(PyError::named(
            "ValueError",
            format!("illegal newline value: {other}"),
        )),
    }
}

struct ValidatedOpen {
    parsed_mode: ParsedMode,
    encoding_norm: Option<String>,
    encoding_display: Option<String>,
    errors: String,
    newline: NewlineMode,
}

fn validate_open_options(opts: &FileOpenOptions<'_>) -> Result<ValidatedOpen> {
    let parsed_mode = parse_mode(opts.mode)?;

    if parsed_mode.is_binary {
        if opts.encoding.is_some() {
            return Err(PyError::named(
                "ValueError",
                "binary mode doesn't take an encoding argument".to_string(),
            ));
        }
        if opts.errors.is_some() {
            return Err(PyError::named(
                "ValueError",
                "binary mode doesn't take an errors argument".to_string(),
            ));
        }
        if opts.newline.is_some() {
            return Err(PyError::named(
                "ValueError",
                "binary mode doesn't take a newline argument".to_string(),
            ));
        }
        return Ok(ValidatedOpen {
            parsed_mode,
            encoding_norm: None,
            encoding_display: None,
            errors: "strict".to_string(),
            newline: NewlineMode::UniversalNoTranslate,
        });
    }

    if opts.buffering == 0 {
        return Err(PyError::named(
            "ValueError",
            "can't have unbuffered text I/O".to_string(),
        ));
    }

    let newline = parse_newline(opts.newline)?;

    let (encoding_norm, encoding_display) = match opts.encoding {
        None => (None, None),
        Some(enc) => {
            // Validate codec name up front via decode_bytes so unknown encodings
            // raise LookupError before touching the filesystem.
            decode_bytes(b"", enc, "strict")?;
            (Some(normalise_encoding(enc)), Some(enc.to_string()))
        }
    };

    let errors = opts.errors.unwrap_or("strict").to_string();

    Ok(ValidatedOpen {
        parsed_mode,
        encoding_norm,
        encoding_display,
        errors,
        newline,
    })
}

/// Backwards-compatible convenience entry point for `open(path, mode, encoding, closefd)`.
pub fn open(path: &str, mode: &str, encoding: Option<&str>, closefd: bool) -> Result<Value> {
    open_with_options(
        path,
        FileOpenOptions {
            mode,
            buffering: -1,
            encoding,
            errors: None,
            newline: None,
            closefd,
        },
    )
}

/// Implements `open(path, mode='r', buffering=-1, encoding=None, errors=None,
/// newline=None, closefd=True)`.
pub fn open_with_options(path: &str, opts: FileOpenOptions<'_>) -> Result<Value> {
    if !opts.closefd {
        return Err(PyError::named(
            "ValueError",
            "Cannot use closefd=False with file name".to_string(),
        ));
    }

    let validated = validate_open_options(&opts)?;
    let pm = &validated.parsed_mode;

    let mut open_opts = std::fs::OpenOptions::new();
    if pm.is_read {
        open_opts.read(true);
        if pm.is_plus {
            open_opts.write(true);
        }
    } else if pm.is_write {
        open_opts.write(true).create(true).truncate(true);
        if pm.is_plus {
            open_opts.read(true);
        }
    } else if pm.is_append {
        open_opts.append(true).create(true);
        if pm.is_plus {
            open_opts.read(true);
        }
    } else if pm.is_create {
        open_opts.write(true).create_new(true);
        if pm.is_plus {
            open_opts.read(true);
        }
    }

    let os_file = open_opts
        .open(path)
        .map_err(|e| PyError::from_io_error(&e, Some(path)))?;

    let state = FileState {
        name: FileName::Path(path.to_string()),
        mode: opts.mode.to_string(),
        closed: false,
        os_file: Some(os_file),
        fd_num: None,
        closefd: true,
        close_fd_fn: None,
        buffering: opts.buffering,
        content: String::new(),
        content_bytes: Vec::new(),
        read_loaded: false,
        read_base_offset: 0,
        pos: 0,
        write_buf_bytes: Vec::new(),
        bom_written: false,
        is_readable_mode: pm.is_read || pm.is_plus,
        is_writable_mode: pm.is_write || pm.is_append || pm.is_create || pm.is_plus,
        is_append: pm.is_append,
        is_binary: pm.is_binary,
        encoding: validated.encoding_norm,
        encoding_display: validated.encoding_display,
        errors: validated.errors,
        newline: validated.newline,
        observed_newlines: ObservedNewlines::default(),
    };
    let state: Box<dyn Any> = Box::new(state);
    Ok(Value::builtin_object(FILE_OPS, state))
}

/// Implements `open(fd, mode='r', buffering=-1, encoding=None, errors=None,
/// newline=None, closefd=True)` for an integer file descriptor.
pub fn open_fd(
    fd: i64,
    file: std::fs::File,
    close_fd_fn: fn(i64) -> std::io::Result<()>,
    opts: FileOpenOptions<'_>,
) -> Result<Value> {
    let validated = validate_open_options(&opts)?;
    let pm = &validated.parsed_mode;

    let state = FileState {
        name: FileName::Fd(fd),
        mode: opts.mode.to_string(),
        closed: false,
        os_file: Some(file),
        fd_num: Some(fd),
        closefd: opts.closefd,
        close_fd_fn: Some(close_fd_fn),
        buffering: opts.buffering,
        content: String::new(),
        content_bytes: Vec::new(),
        read_loaded: false,
        read_base_offset: 0,
        pos: 0,
        write_buf_bytes: Vec::new(),
        bom_written: false,
        is_readable_mode: pm.is_read || pm.is_plus,
        is_writable_mode: pm.is_write || pm.is_append || pm.is_create || pm.is_plus,
        is_append: pm.is_append,
        is_binary: pm.is_binary,
        encoding: validated.encoding_norm,
        encoding_display: validated.encoding_display,
        errors: validated.errors,
        newline: validated.newline,
        observed_newlines: ObservedNewlines::default(),
    };
    let state: Box<dyn Any> = Box::new(state);
    Ok(Value::builtin_object(FILE_OPS, state))
}

/// Decode raw file bytes to a `String` using the given encoding and error handler.
fn decode_text_bytes(raw: &[u8], enc: &str, errors: &str) -> Result<String> {
    let val = decode_bytes(raw, enc, errors)?;
    match val.kind() {
        pyrust_core::ValueKind::Str(s) => Ok(s.to_string()),
        _ => Ok(String::new()),
    }
}

/// Encode a text string to bytes using the file's encoding and error handler.
/// When `include_bom` is false, BOM-emitting codecs (`utf-8-sig`, `utf-16`,
/// `utf-32`) omit their leading BOM (for subsequent writes after the first).
fn encode_text_to_bytes(s: &str, enc: &str, errors: &str, include_bom: bool) -> Result<Vec<u8>> {
    let effective_enc = if !include_bom {
        match enc {
            "utf-8-sig" | "utf8sig" | "utf-8sig" => "utf-8",
            "utf-16" | "utf16" => "utf-16-le",
            "utf-32" | "utf32" => "utf-32-le",
            other => other,
        }
    } else {
        enc
    };
    let val = encode_str_to_bytes(s, effective_enc, errors)?;
    match val.kind() {
        pyrust_core::ValueKind::Bytes(rc) => Ok(rc.as_slice().to_vec()),
        _ => Ok(Vec::new()),
    }
}

/// Translate output `\n` according to `NewlineMode`.
/// Operates on UTF-8 / CESU-8 bytes directly so lone surrogates from
/// `surrogateescape` never touch `str::chars()`.
fn translate_output_newlines(s: &str, mode: NewlineMode) -> String {
    let replacement: Option<&[u8]> = match mode {
        NewlineMode::UniversalTranslate => {
            if cfg!(windows) {
                Some(b"\r\n")
            } else {
                None
            }
        }
        NewlineMode::UniversalNoTranslate | NewlineMode::Lf => None,
        NewlineMode::Cr => Some(b"\r"),
        NewlineMode::CrLf => Some(b"\r\n"),
    };
    let Some(repl) = replacement else {
        return s.to_string();
    };
    let bytes = s.as_bytes();
    if !bytes.contains(&b'\n') {
        return s.to_string();
    }
    let mut out = Vec::with_capacity(bytes.len() + 16);
    for &b in bytes {
        if b == b'\n' {
            out.extend_from_slice(repl);
        } else {
            out.push(b);
        }
    }
    // SAFETY: `s` is valid UTF-8 / CESU-8, and replacing ASCII `0x0A` with
    // ASCII `\r` or `\r\n` preserves byte validity identically.
    unsafe { String::from_utf8_unchecked(out) }
}

/// Translate input `\r\n` and `\r` to `\n` for `newline=None` (universal
/// newlines) and record observed line endings. Safe on CESU-8 strings.
fn translate_universal_input_newlines(s: &str, observed: &mut ObservedNewlines) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    observed.seen_crlf = true;
                    out.push(b'\n');
                    i += 2;
                } else {
                    observed.seen_cr = true;
                    out.push(b'\n');
                    i += 1;
                }
            }
            b'\n' => {
                observed.seen_lf = true;
                out.push(b'\n');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    // SAFETY: `s` is valid UTF-8 / CESU-8 and only ASCII `\r`/`\n` are rewritten.
    unsafe { String::from_utf8_unchecked(out) }
}

/// Record observed line endings without modifying the text (for `newline=""`).
fn scan_observed_newlines(s: &str, observed: &mut ObservedNewlines) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    observed.seen_crlf = true;
                    i += 2;
                } else {
                    observed.seen_cr = true;
                    i += 1;
                }
            }
            b'\n' => {
                observed.seen_lf = true;
                i += 1;
            }
            _ => i += 1,
        }
    }
}

/// Byte width of one UTF-8 / CESU-8 codepoint starting at `first_byte`.
#[inline]
fn cesu8_char_width(first_byte: u8) -> usize {
    if first_byte < 0x80 {
        1
    } else if first_byte < 0xE0 {
        2
    } else if first_byte < 0xF0 {
        3
    } else {
        4
    }
}

/// Count codepoints in a UTF-8 / CESU-8 byte slice without calling `str::chars()`.
fn cesu8_codepoint_count(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut count = 0;
    while i < bytes.len() {
        i += cesu8_char_width(bytes[i]);
        count += 1;
    }
    count
}

/// Advance `start` by up to `max_chars` codepoints within `bytes`, returning
/// the new byte offset (clamped to `bytes.len()`).
fn advance_cesu8_chars(bytes: &[u8], start: usize, max_chars: usize) -> usize {
    let mut idx = start;
    let mut chars = 0;
    while idx < bytes.len() && chars < max_chars {
        let w = cesu8_char_width(bytes[idx]);
        if idx + w > bytes.len() {
            return bytes.len();
        }
        idx += w;
        chars += 1;
    }
    idx
}

fn flush_write_buf(s: &mut FileState) -> Result<()> {
    if s.write_buf_bytes.is_empty() {
        return Ok(());
    }
    let err_path = s.error_path().map(str::to_owned);
    if let Some(ref mut f) = s.os_file {
        f.write_all(&s.write_buf_bytes)
            .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))?;
        f.flush()
            .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))?;
    }
    s.write_buf_bytes.clear();
    // After writing, any cached read buffer is stale.
    s.read_loaded = false;
    s.content.clear();
    s.content_bytes.clear();
    s.pos = 0;
    Ok(())
}

fn ensure_read_loaded(s: &mut FileState) -> Result<()> {
    if s.read_loaded {
        return Ok(());
    }
    flush_write_buf(s)?;
    let err_path = s.error_path().map(str::to_owned);
    let mut raw = Vec::new();
    let base_offset = if let Some(ref mut f) = s.os_file {
        let off = f.stream_position().unwrap_or(0);
        f.read_to_end(&mut raw)
            .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))?;
        off
    } else {
        0
    };
    if s.is_binary {
        s.content_bytes = raw;
    } else {
        let enc = s.encoding.as_deref().unwrap_or("utf-8");
        let decoded = decode_text_bytes(&raw, enc, &s.errors)?;
        s.content = match s.newline {
            NewlineMode::UniversalTranslate => {
                translate_universal_input_newlines(&decoded, &mut s.observed_newlines)
            }
            NewlineMode::UniversalNoTranslate => {
                scan_observed_newlines(&decoded, &mut s.observed_newlines);
                decoded
            }
            NewlineMode::Lf | NewlineMode::Cr | NewlineMode::CrLf => decoded,
        };
    }
    s.read_base_offset = base_offset;
    s.pos = 0;
    s.read_loaded = true;
    Ok(())
}

fn closed_file_error() -> PyError {
    PyError::named("ValueError", "I/O operation on closed file.".to_string())
}

fn call_file_method_inner(state: &BuiltinState, method: &str, args: &[Value]) -> Result<Value> {
    match method {
        "__enter__" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<FileState>()
                .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            drop(borrow);
            Ok(Value::builtin_object_shared(FILE_OPS, Rc::clone(state)))
        }
        "__exit__" => {
            close_file(state)?;
            Ok(Value::none())
        }
        "__iter__" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<FileState>()
                .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            drop(borrow);
            Ok(Value::builtin_object_shared(FILE_OPS, Rc::clone(state)))
        }
        "__next__" => match read_line_value(state, None)? {
            Some(v) => Ok(v),
            None => Err(PyError::named("StopIteration", String::new())),
        },
        "close" => {
            close_file(state)?;
            Ok(Value::none())
        }
        "flush" => {
            let mut borrow = state.borrow_mut();
            let s = borrow
                .downcast_mut::<FileState>()
                .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            flush_write_buf(s)?;
            Ok(Value::none())
        }
        "readable" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<FileState>()
                .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(s.is_readable()))
        }
        "writable" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<FileState>()
                .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(s.is_writable()))
        }
        "seekable" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<FileState>()
                .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(true))
        }
        "isatty" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<FileState>()
                .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
            if s.closed {
                return Err(closed_file_error());
            }
            Ok(Value::bool_(false))
        }
        "fileno" => file_fileno(state),
        "tell" => file_tell(state),
        "seek" => file_seek(state, args),
        "truncate" => file_truncate(state, args),
        "read" => file_read(state, args),
        "readline" => {
            let limit = parse_optional_size_arg(args)?;
            let v = read_line_value(state, limit)?;
            Ok(v.unwrap_or_else(|| {
                let borrow = state.borrow();
                if let Some(s) = borrow.downcast_ref::<FileState>()
                    && s.is_binary
                {
                    return Value::bytes(Vec::new());
                }
                Value::string(String::new())
            }))
        }
        "readlines" => {
            let mut out = Vec::new();
            while let Some(v) = read_line_value(state, None)? {
                out.push(v);
            }
            Ok(Value::list(out))
        }
        "write" => file_write(state, args),
        "writelines" => file_writelines(state, args),
        _ => Err(PyError::named(
            "AttributeError",
            format!("'{TYPE_NAME}' object has no attribute '{method}'"),
        )),
    }
}

fn parse_optional_size_arg(args: &[Value]) -> Result<Option<usize>> {
    match args.first().map(|v| v.kind()) {
        None | Some(ValueKind::None) => Ok(None),
        Some(ValueKind::Int(n)) if n < 0 => Ok(None),
        Some(ValueKind::Int(n)) => Ok(Some(n as usize)),
        Some(ValueKind::Bool(b)) => Ok(Some(usize::from(b))),
        _ => Err(PyError::named(
            "TypeError",
            "argument should be integer or None".to_string(),
        )),
    }
}

fn file_fileno(state: &BuiltinState) -> Result<Value> {
    let borrow = state.borrow();
    let s = borrow
        .downcast_ref::<FileState>()
        .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Err(closed_file_error());
    }
    if let Some(fd) = s.fd_num {
        return Ok(Value::int(fd));
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if let Some(ref f) = s.os_file {
            return Ok(Value::int(f.as_raw_fd() as i64));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        if let Some(ref f) = s.os_file {
            // Return a positive non-stdio integer descriptor handle proxy.
            let raw = f.as_raw_handle() as usize;
            return Ok(Value::int(((raw % 65000) + 3) as i64));
        }
    }
    Err(PyError::named(
        "io.UnsupportedOperation",
        "fileno".to_string(),
    ))
}

fn file_tell(state: &BuiltinState) -> Result<Value> {
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Err(closed_file_error());
    }
    if s.read_loaded {
        return Ok(Value::int(s.read_base_offset as i64 + s.pos as i64));
    }
    let err_path = s.error_path().map(str::to_owned);
    let base_pos = if let Some(ref mut f) = s.os_file {
        f.stream_position()
            .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))? as i64
    } else {
        0
    };
    Ok(Value::int(base_pos + s.write_buf_bytes.len() as i64))
}

fn file_seek(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let offset = args
        .first()
        .and_then(|v| match v.kind() {
            ValueKind::Int(n) => Some(n),
            ValueKind::Bool(b) => Some(i64::from(b)),
            _ => None,
        })
        .ok_or_else(|| PyError::named("TypeError", "seek() argument must be int".to_string()))?;
    let whence = args
        .get(1)
        .and_then(|v| match v.kind() {
            ValueKind::Int(n) => Some(n),
            ValueKind::Bool(b) => Some(i64::from(b)),
            _ => None,
        })
        .unwrap_or(0);
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Err(closed_file_error());
    }

    flush_write_buf(s)?;

    if !s.is_binary {
        match whence {
            0 => {
                if offset < 0 {
                    return Err(PyError::named(
                        "ValueError",
                        format!("negative seek position {offset}"),
                    ));
                }
            }
            1 => {
                if offset != 0 {
                    return Err(PyError::named(
                        "io.UnsupportedOperation",
                        "can't do nonzero cur-relative seeks".to_string(),
                    ));
                }
            }
            2 => {
                if offset != 0 {
                    return Err(PyError::named(
                        "io.UnsupportedOperation",
                        "can't do nonzero end-relative seeks".to_string(),
                    ));
                }
            }
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    format!("invalid whence ({whence}, should be 0, 1 or 2)"),
                ));
            }
        }
    }

    let err_path = s.error_path().map(str::to_owned);
    // Compute the logical current position before invalidating any read buffer,
    // because `ensure_read_loaded` drained `os_file` to EOF.
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
            if s.read_loaded {
                let logical_cur = s.read_base_offset as i64 + s.pos as i64;
                let target = logical_cur + offset;
                if target < 0 {
                    return Err(PyError::named(
                        "ValueError",
                        format!("negative seek position {target}"),
                    ));
                }
                SeekFrom::Start(target as u64)
            } else {
                SeekFrom::Current(offset)
            }
        }
        2 => SeekFrom::End(offset),
        _ => {
            return Err(PyError::named(
                "ValueError",
                format!("invalid whence ({whence}, should be 0, 1 or 2)"),
            ));
        }
    };
    let new_pos = if let Some(ref mut f) = s.os_file {
        f.seek(seek_from)
            .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))? as i64
    } else {
        0
    };
    s.read_loaded = false;
    s.read_base_offset = new_pos as u64;
    s.content.clear();
    s.content_bytes.clear();
    s.pos = 0;
    Ok(Value::int(new_pos))
}

fn file_truncate(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Err(closed_file_error());
    }
    if !s.is_writable() {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "File not open for writing".to_string(),
        ));
    }
    flush_write_buf(s)?;
    let err_path = s.error_path().map(str::to_owned);
    let target_len: u64 = match args.first().map(|v| v.kind()) {
        None | Some(ValueKind::None) => {
            if let Some(ref mut f) = s.os_file {
                f.stream_position()
                    .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))?
            } else {
                0
            }
        }
        Some(ValueKind::Int(n)) if n >= 0 => n as u64,
        Some(ValueKind::Int(n)) => {
            return Err(PyError::named(
                "ValueError",
                format!("Negative size value {n}"),
            ));
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                "truncate() argument must be int or None".to_string(),
            ));
        }
    };
    if let Some(ref mut f) = s.os_file {
        f.set_len(target_len)
            .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))?;
    }
    s.read_loaded = false;
    Ok(Value::int(target_len as i64))
}

fn file_read(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let n = match args.first().map(|v| v.kind()) {
        None | Some(ValueKind::None) => None,
        Some(ValueKind::Int(n)) => Some(n),
        Some(ValueKind::Bool(b)) => Some(i64::from(b)),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "argument should be integer or None".to_string(),
            ));
        }
    };
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Err(closed_file_error());
    }
    if !s.is_readable() {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not readable".to_string(),
        ));
    }

    // Unbuffered binary I/O (`buffering=0`): read directly from the OS handle
    // without reading ahead into an internal buffer.
    if s.is_binary && s.buffering == 0 {
        flush_write_buf(s)?;
        let err_path = s.error_path().map(str::to_owned);
        let Some(ref mut f) = s.os_file else {
            return Ok(Value::bytes(Vec::new()));
        };
        let mut buf = Vec::new();
        match n {
            Some(count) if count >= 0 => {
                buf.resize(count as usize, 0);
                let got = f
                    .read(&mut buf)
                    .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))?;
                buf.truncate(got);
            }
            _ => {
                f.read_to_end(&mut buf)
                    .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))?;
            }
        }
        return Ok(Value::bytes(buf));
    }

    ensure_read_loaded(s)?;

    if s.is_binary {
        let remaining = &s.content_bytes[s.pos..];
        let take = match n {
            Some(n) if n >= 0 => remaining.len().min(n as usize),
            _ => remaining.len(),
        };
        let out = remaining[..take].to_vec();
        s.pos += take;
        return Ok(Value::bytes(out));
    }

    let bytes = s.content.as_bytes();
    let end = match n {
        Some(n) if n >= 0 => advance_cesu8_chars(bytes, s.pos, n as usize),
        _ => bytes.len(),
    };
    let out = s.content[s.pos..end].to_string();
    s.pos = end;
    Ok(Value::string(out))
}

fn extract_bytes_like(value: &Value) -> Option<Vec<u8>> {
    match value.kind() {
        ValueKind::Bytes(rc) => Some(rc.as_slice().to_vec()),
        _ => crate::bytearray::as_bytearray_snapshot(value),
    }
}

fn write_text_chunk(st: &mut FileState, s: &str) -> Result<i64> {
    let char_len = cesu8_codepoint_count(s) as i64;
    let translated = translate_output_newlines(s, st.newline);
    let enc = st.encoding.as_deref().unwrap_or("utf-8");
    let include_bom = !st.bom_written && !st.is_append;
    let encoded = encode_text_to_bytes(&translated, enc, &st.errors, include_bom)?;
    st.bom_written = true;
    st.write_buf_bytes.extend_from_slice(&encoded);
    let should_flush = (st.buffering == 1 && (s.contains('\n') || s.contains('\r')))
        || (st.buffering > 1 && st.write_buf_bytes.len() >= st.buffering as usize);
    if should_flush {
        flush_write_buf(st)?;
    }
    Ok(char_len)
}

fn write_binary_chunk(st: &mut FileState, data: &[u8]) -> Result<i64> {
    let len = data.len() as i64;
    if st.buffering == 0 {
        let err_path = st.error_path().map(str::to_owned);
        if let Some(ref mut f) = st.os_file {
            f.write_all(data)
                .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))?;
        }
        st.read_loaded = false;
        return Ok(len);
    }
    st.write_buf_bytes.extend_from_slice(data);
    if st.buffering > 1 && st.write_buf_bytes.len() >= st.buffering as usize {
        flush_write_buf(st)?;
    }
    Ok(len)
}

fn file_write(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let mut borrow = state.borrow_mut();
    let st = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
    if st.closed {
        return Err(closed_file_error());
    }
    if !st.is_writable() {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not writable".to_string(),
        ));
    }
    if st.is_binary {
        let Some(arg) = args.first() else {
            return Err(PyError::named(
                "TypeError",
                "write() takes exactly one argument (0 given)".to_string(),
            ));
        };
        let Some(data) = extract_bytes_like(arg) else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(arg)
                ),
            ));
        };
        let written = write_binary_chunk(st, &data)?;
        Ok(Value::int(written))
    } else {
        let arg = args.first();
        match arg.map(|v| v.kind()) {
            Some(ValueKind::Bytes(_)) => Err(PyError::named(
                "TypeError",
                "write() argument must be str, not bytes".to_string(),
            )),
            _ => {
                let s = arg
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .ok_or_else(|| {
                        PyError::named("TypeError", "write() argument must be str".to_string())
                    })?;
                let written = write_text_chunk(st, &s)?;
                Ok(Value::int(written))
            }
        }
    }
}

fn file_writelines(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let lines = match args.first().map(|v| v.kind()) {
        Some(ValueKind::List(items)) => items.clone(),
        Some(ValueKind::Tuple(items)) => items.to_vec(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "writelines() argument must be a list or tuple of str".to_string(),
            ));
        }
    };
    let mut borrow = state.borrow_mut();
    let st = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
    if st.closed {
        return Err(closed_file_error());
    }
    if !st.is_writable() {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not writable".to_string(),
        ));
    }
    if st.is_binary {
        for v in lines {
            let Some(data) = extract_bytes_like(&v) else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "a bytes-like object is required, not '{}'",
                        pyrust_core::builtin_type_name(&v)
                    ),
                ));
            };
            write_binary_chunk(st, &data)?;
        }
    } else {
        for v in lines {
            match v.kind() {
                ValueKind::Str(s) => {
                    let owned = s.to_string();
                    write_text_chunk(st, &owned)?;
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "writelines() requires str items".to_string(),
                    ));
                }
            }
        }
    }
    Ok(Value::none())
}

/// Find the end byte offset of the next line starting at `start` in `bytes`
/// according to `newline` mode (and optional `limit` codepoints).
fn find_text_line_end(
    bytes: &[u8],
    start: usize,
    newline: NewlineMode,
    limit: Option<usize>,
) -> usize {
    let max_end = match limit {
        Some(n) => advance_cesu8_chars(bytes, start, n),
        None => bytes.len(),
    };
    let mut i = start;
    match newline {
        NewlineMode::UniversalTranslate | NewlineMode::Lf => {
            while i < max_end {
                if bytes[i] == b'\n' {
                    return i + 1;
                }
                i += 1;
            }
            max_end
        }
        NewlineMode::UniversalNoTranslate => {
            while i < max_end {
                if bytes[i] == b'\n' {
                    return i + 1;
                }
                if bytes[i] == b'\r' {
                    if i + 1 < max_end && bytes[i + 1] == b'\n' {
                        return i + 2;
                    }
                    return i + 1;
                }
                i += 1;
            }
            max_end
        }
        NewlineMode::Cr => {
            while i < max_end {
                if bytes[i] == b'\r' {
                    return i + 1;
                }
                i += 1;
            }
            max_end
        }
        NewlineMode::CrLf => {
            while i < max_end {
                if bytes[i] == b'\r' && i + 1 < max_end && bytes[i + 1] == b'\n' {
                    return i + 2;
                }
                i += 1;
            }
            max_end
        }
    }
}

/// Read the next line from a file and return it as the appropriate Value type
/// (`bytes` in binary mode, `str` in text mode). Returns `None` at EOF.
fn read_line_value(state: &BuiltinState, limit: Option<usize>) -> Result<Option<Value>> {
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Err(closed_file_error());
    }
    if !s.is_readable() {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not readable".to_string(),
        ));
    }

    if s.is_binary && s.buffering == 0 {
        flush_write_buf(s)?;
        let err_path = s.error_path().map(str::to_owned);
        let Some(ref mut f) = s.os_file else {
            return Ok(None);
        };
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            if let Some(lim) = limit
                && line.len() >= lim
            {
                break;
            }
            let got = f
                .read(&mut byte)
                .map_err(|e| PyError::from_io_error(&e, err_path.as_deref()))?;
            if got == 0 {
                break;
            }
            line.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        if line.is_empty() {
            return Ok(None);
        }
        return Ok(Some(Value::bytes(line)));
    }

    ensure_read_loaded(s)?;

    if s.is_binary {
        let bytes = &s.content_bytes;
        if s.pos >= bytes.len() {
            return Ok(None);
        }
        let max_end = match limit {
            Some(lim) => (s.pos + lim).min(bytes.len()),
            None => bytes.len(),
        };
        let mut end = s.pos;
        while end < max_end && bytes[end] != b'\n' {
            end += 1;
        }
        if end < max_end {
            end += 1;
        }
        let line = bytes[s.pos..end].to_vec();
        s.pos = end;
        Ok(Some(Value::bytes(line)))
    } else {
        let bytes = s.content.as_bytes();
        if s.pos >= bytes.len() {
            return Ok(None);
        }
        let end = find_text_line_end(bytes, s.pos, s.newline, limit);
        let line = s.content[s.pos..end].to_string();
        s.pos = end;
        Ok(Some(Value::string(line)))
    }
}

fn close_file(state: &BuiltinState) -> Result<()> {
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Ok(());
    }
    let flush_result = if s.is_writable() {
        flush_write_buf(s)
    } else {
        Ok(())
    };
    s.os_file = None;
    s.closed = true;
    s.write_buf_bytes.clear();
    if s.closefd
        && let (Some(fd), Some(close_fn)) = (s.fd_num, s.close_fd_fn)
    {
        let _ = close_fn(fd);
    }
    flush_result
}

/// Returns true if `name` is a method on a file object. Used by `hasattr`.
pub fn has_method(name: &str) -> bool {
    matches!(
        name,
        "__enter__"
            | "__exit__"
            | "__iter__"
            | "__next__"
            | "close"
            | "fileno"
            | "flush"
            | "isatty"
            | "read"
            | "readable"
            | "readline"
            | "readlines"
            | "seek"
            | "seekable"
            | "tell"
            | "truncate"
            | "writable"
            | "write"
            | "writelines"
    )
}
