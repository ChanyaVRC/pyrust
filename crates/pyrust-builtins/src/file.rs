//! `open()` and file I/O — relocated from `pyrust-core` Tier 1.
//!
//! Built-in file objects are not a Tier 1 type: they have no literal syntax
//! and no compile-time-specializable payload (just a `Box<dyn Any>` already).
//! All state and operations live here; the VM sees only a generic
//! `BuiltinObject` whose `BuiltinTypeOps` impl dispatches through this module.

use std::any::Any;
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

pub struct FileState {
    pub path: String,
    pub mode: String,
    pub closed: bool,
    /// For text read modes: the full file contents; we read eagerly at open time.
    pub content: String,
    /// For binary read modes: the full file contents as raw bytes.
    pub content_bytes: Vec<u8>,
    /// Read cursor (in bytes for both text and binary mode).
    pub pos: usize,
    /// For text write modes: buffered text to flush on `close()`.
    pub write_buf: String,
    /// For binary write modes: buffered bytes to flush on `close()`.
    pub write_buf_bytes: Vec<u8>,
    pub is_write: bool,
    pub is_append: bool,
    /// Whether the file was opened in binary mode (`'b'` in mode string).
    pub is_binary: bool,
    /// Encoding for text mode (e.g. "utf-8", "ascii", "latin-1").  Always
    /// `None` for binary mode.  `None` in text mode means locale default
    /// (pyrust uses UTF-8 as the locale default).
    /// This is the normalised (lowercase, hyphens) form used for codec routing.
    pub encoding: Option<String>,
    /// The original user-supplied encoding string (exact case as passed to `open()`).
    /// Used for `repr()` and the `.encoding` attribute to match CPython behaviour
    /// of preserving the user-supplied name verbatim.
    pub encoding_display: Option<String>,
}

impl FileState {
    fn is_readable(&self) -> bool {
        !self.is_write && !self.is_append
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
            if s.is_binary {
                format!("<_io.BufferedReader name='{}'>", s.path)
            } else {
                let enc = s.encoding_display.as_deref().unwrap_or("UTF-8");
                format!(
                    "<_io.TextIOWrapper name='{}' mode='{}' encoding='{}'>",
                    s.path, s.mode, enc
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
        read_line_value(state)
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
            "name" => Some(Value::string(s.path.clone())),
            "mode" => Some(Value::string(s.mode.clone())),
            "closed" => Some(Value::bool_(s.closed)),
            "encoding" => {
                if s.is_binary {
                    None
                } else {
                    let enc = s.encoding_display.as_deref().unwrap_or("UTF-8");
                    Some(Value::string(enc))
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

/// Normalise an encoding name to the canonical lowercase form used internally
/// (e.g. `"UTF-8"` → `"utf-8"`, `"UTF_8"` → `"utf-8"`).
fn normalise_encoding(enc: &str) -> String {
    enc.to_ascii_lowercase().replace('_', "-")
}

/// Implements `open(path, mode='r', buffering=-1, encoding=None, errors=None,
/// newline=None, closefd=True, opener=None)`.
///
/// Returns a `BuiltinObject` carrying the file state plus the static `FileOps`
/// dispatch table.
///
/// `encoding` is `None` when not supplied (defaults to UTF-8 for text mode).
/// `closefd` must be `true` when `path` is a filename string; `false` is only
/// valid when passing an integer file descriptor (not yet supported by pyrust).
pub fn open(path: &str, mode: &str, encoding: Option<&str>, closefd: bool) -> Result<Value> {
    // CPython raises ValueError when closefd=False and a filename is given.
    if !closefd {
        return Err(PyError::named(
            "ValueError",
            "closefd=False with a file path is not supported".to_string(),
        ));
    }

    for c in mode.chars() {
        if !matches!(c, 'r' | 'w' | 'a' | 'b' | 't' | '+') {
            return Err(PyError::named(
                "ValueError",
                format!("invalid mode: '{mode}'"),
            ));
        }
    }
    let is_write = mode.contains('w');
    let is_append = mode.contains('a');
    let is_read = mode.contains('r') || (!is_write && !is_append);
    let is_binary = mode.contains('b');

    // Validate the encoding name early (before opening the file) so we get a
    // clean LookupError rather than a confusing IO error.
    // encoding_norm: lowercased+hyphens form used for codec routing.
    // encoding_display: exact user-supplied string preserved for .encoding attr
    //   and repr(), matching CPython which returns the verbatim user input.
    let (encoding_norm, encoding_display): (Option<String>, Option<String>) = if is_binary {
        // Binary mode must not have an encoding.
        if encoding.is_some() {
            return Err(PyError::named(
                "ValueError",
                "binary mode doesn't take an encoding argument".to_string(),
            ));
        }
        (None, None)
    } else {
        match encoding {
            None => (None, None),
            Some(enc) => {
                let norm = normalise_encoding(enc);
                // Validate the name by attempting a dummy decode of empty bytes.
                // This surfaces LookupError for unknown encodings before IO.
                match norm.as_str() {
                    "utf-8" | "utf8" | "u8" | "utf" | "utf-8-sig" | "utf8sig" | "utf-8sig"
                    | "ascii" | "us-ascii" | "646" | "latin-1" | "iso-8859-1" | "8859"
                    | "cp819" | "latin1" | "l1" | "utf-16" | "utf16" => {}
                    _ => {
                        return Err(PyError::named(
                            "LookupError",
                            format!("unknown encoding: {enc}"),
                        ));
                    }
                }
                (Some(norm), Some(enc.to_string()))
            }
        }
    };

    let mut content = String::new();
    let mut content_bytes = Vec::new();
    if is_read {
        if is_binary {
            content_bytes =
                std::fs::read(path).map_err(|e| PyError::from_io_error(&e, Some(path)))?;
        } else {
            let raw_bytes =
                std::fs::read(path).map_err(|e| PyError::from_io_error(&e, Some(path)))?;
            let enc = encoding_norm.as_deref().unwrap_or("utf-8");
            let decoded = decode_text_bytes(&raw_bytes, enc)?;
            // On Windows, text mode strips \r from \r\n (universal newlines).
            #[cfg(windows)]
            {
                content = decoded.replace("\r\n", "\n");
            }
            #[cfg(not(windows))]
            {
                content = decoded;
            }
        }
    }
    let state = FileState {
        path: path.to_string(),
        mode: mode.to_string(),
        closed: false,
        content,
        content_bytes,
        pos: 0,
        write_buf: String::new(),
        write_buf_bytes: Vec::new(),
        is_write,
        is_append,
        is_binary,
        encoding: encoding_norm,
        encoding_display,
    };
    let state: Box<dyn Any> = Box::new(state);
    Ok(Value::builtin_object(FILE_OPS, state))
}

/// Decode raw file bytes to a `String` using the given normalised encoding name.
/// Only called for text-mode opens; binary opens skip this entirely.
fn decode_text_bytes(raw: &[u8], enc: &str) -> Result<String> {
    // utf-8-sig: strip leading BOM (U+FEFF encoded as EF BB BF) if present.
    if matches!(enc, "utf-8-sig" | "utf8sig" | "utf-8sig") {
        let stripped = if raw.starts_with(b"\xEF\xBB\xBF") {
            &raw[3..]
        } else {
            raw
        };
        return match std::str::from_utf8(stripped) {
            Ok(s) => Ok(s.to_string()),
            Err(e) => {
                let start = e.valid_up_to();
                let end = start + e.error_len().unwrap_or(stripped.len() - start);
                Err(PyError::UnicodeDecodeError {
                    encoding: "utf-8-sig".to_string(),
                    object: stripped.to_vec(),
                    start,
                    end,
                    reason: "invalid start byte".to_string(),
                })
            }
        };
    }
    // utf-16: use Rust's std to parse both LE and BE variants via the BOM.
    if matches!(enc, "utf-16" | "utf16") {
        // Try to decode UTF-16 via the BOM; if no BOM, assume native endian.
        // We pair bytes and decode; Rust doesn't have built-in UTF-16, so we
        // do it manually.
        let s = decode_utf16(raw)?;
        return Ok(s);
    }
    // For all other encodings, delegate to the existing decode_bytes helper
    // (which returns a `Value::Str`; we unwrap the inner string).
    let val = decode_bytes(raw, enc, "strict")?;
    match val.kind() {
        pyrust_core::ValueKind::Str(s) => Ok(s.to_string()),
        _ => Ok(String::new()),
    }
}

/// Minimal UTF-16 decoder: handles BOM-prefixed streams (LE or BE).
/// Falls back to native endian when no BOM is present.
fn decode_utf16(raw: &[u8]) -> Result<String> {
    if !raw.len().is_multiple_of(2) {
        // Odd byte count → truncated sequence.
        return Err(PyError::named(
            "UnicodeDecodeError",
            "'utf-16' codec can't decode bytes: truncated data".to_string(),
        ));
    }
    // Detect BOM.
    let (le, payload) = if raw.starts_with(b"\xFF\xFE") {
        (true, &raw[2..])
    } else if raw.starts_with(b"\xFE\xFF") {
        (false, &raw[2..])
    } else {
        // No BOM: assume LE (matches CPython's default on little-endian hosts).
        (true, raw)
    };
    let words: Vec<u16> = payload
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16(&words).map_err(|_| {
        PyError::named(
            "UnicodeDecodeError",
            "'utf-16' codec can't decode bytes: invalid data".to_string(),
        )
    })
}

/// Encode a text string to bytes using the file's encoding for writing.
/// Returns `Err` if the encoding is unsupported or the string can't be encoded.
fn encode_text_to_bytes(s: &str, enc: &str) -> Result<Vec<u8>> {
    // utf-8-sig: prepend BOM only when writing the first flush.
    // Since we buffer the whole write_buf and flush at close time, we prepend
    // the BOM to the entire output (matching CPython's behaviour for a freshly
    // opened 'w' file with encoding='utf-8-sig').
    if matches!(enc, "utf-8-sig" | "utf8sig" | "utf-8sig") {
        let mut out = Vec::with_capacity(3 + s.len());
        out.extend_from_slice(b"\xEF\xBB\xBF");
        out.extend_from_slice(s.as_bytes());
        return Ok(out);
    }
    if matches!(enc, "utf-16" | "utf16") {
        // Write UTF-16 LE with BOM.
        let mut out: Vec<u8> = Vec::with_capacity(2 + s.len() * 2);
        out.extend_from_slice(b"\xFF\xFE"); // LE BOM
        for c in s.encode_utf16() {
            out.extend_from_slice(&c.to_le_bytes());
        }
        return Ok(out);
    }
    // Delegate to encode_str_to_bytes for utf-8 / ascii / latin-1.
    let val = encode_str_to_bytes(s, enc, "strict")?;
    match val.kind() {
        pyrust_core::ValueKind::Bytes(rc) => Ok(rc.as_slice().to_vec()),
        _ => Ok(Vec::new()),
    }
}

fn call_file_method_inner(state: &BuiltinState, method: &str, args: &[Value]) -> Result<Value> {
    match method {
        "__enter__" => Ok(Value::builtin_object_shared(FILE_OPS, Rc::clone(state))),
        "__exit__" => {
            close_file(state)?;
            Ok(Value::none())
        }
        "__iter__" => Ok(Value::builtin_object_shared(FILE_OPS, Rc::clone(state))),
        "__next__" => match read_line_value(state)? {
            Some(v) => Ok(v),
            None => Err(PyError::named("StopIteration", String::new())),
        },
        "close" => {
            close_file(state)?;
            Ok(Value::none())
        }
        "flush" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<FileState>()
                .ok_or_else(|| PyError::Runtime("internal: bad file state".to_string()))?;
            if s.closed {
                return Err(PyError::named(
                    "ValueError",
                    "I/O operation on closed file.".to_string(),
                ));
            }
            Ok(Value::none())
        }
        "tell" => {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<FileState>()
                .ok_or_else(|| PyError::Runtime("internal: bad file state".to_string()))?;
            if s.closed {
                return Err(PyError::named(
                    "ValueError",
                    "I/O operation on closed file.".to_string(),
                ));
            }
            let pos = if s.is_readable() {
                s.pos
            } else if s.is_binary {
                s.write_buf_bytes.len()
            } else {
                s.write_buf.len()
            };
            Ok(Value::int(pos as i64))
        }
        "seek" => file_seek(state, args),
        "read" => file_read(state, args),
        "readline" => {
            let v = read_line_value(state)?;
            Ok(v.unwrap_or_else(|| {
                // EOF: return empty bytes or empty str depending on mode
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
            while let Some(v) = read_line_value(state)? {
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

fn file_seek(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let offset = args
        .first()
        .and_then(|v| match v.kind() {
            ValueKind::Int(n) => Some(n),
            _ => None,
        })
        .ok_or_else(|| PyError::named("TypeError", "seek() argument must be int".to_string()))?;
    let whence = args
        .get(1)
        .and_then(|v| match v.kind() {
            ValueKind::Int(n) => Some(n),
            _ => None,
        })
        .unwrap_or(0);
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::Runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Err(PyError::named(
            "ValueError",
            "I/O operation on closed file.".to_string(),
        ));
    }
    if s.is_readable() {
        let content_len = if s.is_binary {
            s.content_bytes.len()
        } else {
            s.content.len()
        };
        let new_pos = if s.is_binary {
            // Binary mode allows all seek forms with nonzero offsets
            match whence {
                0 => {
                    if offset < 0 {
                        return Err(PyError::named(
                            "ValueError",
                            format!("negative seek position {offset}"),
                        ));
                    }
                    (offset as usize).min(content_len)
                }
                1 => {
                    let new = s.pos as i64 + offset;
                    if new < 0 {
                        return Err(PyError::named(
                            "ValueError",
                            format!("negative seek position {new}"),
                        ));
                    }
                    (new as usize).min(content_len)
                }
                2 => {
                    let new = content_len as i64 + offset;
                    if new < 0 {
                        return Err(PyError::named(
                            "ValueError",
                            format!("negative seek position {new}"),
                        ));
                    }
                    (new as usize).min(content_len)
                }
                _ => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("invalid whence ({whence}, should be 0, 1 or 2)"),
                    ));
                }
            }
        } else {
            match whence {
                0 => {
                    if offset < 0 {
                        return Err(PyError::named(
                            "ValueError",
                            format!("negative seek position {offset}"),
                        ));
                    }
                    (offset as usize).min(content_len)
                }
                1 => {
                    // Text mode only allows seek(0, 1)
                    if offset != 0 {
                        return Err(PyError::named(
                            "io.UnsupportedOperation",
                            "can't do nonzero cur-relative seeks".to_string(),
                        ));
                    }
                    s.pos
                }
                2 => {
                    // Text mode only allows seek(0, 2)
                    if offset != 0 {
                        return Err(PyError::named(
                            "io.UnsupportedOperation",
                            "can't do nonzero end-relative seeks".to_string(),
                        ));
                    }
                    content_len
                }
                _ => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("invalid whence ({whence}, should be 0, 1 or 2)"),
                    ));
                }
            }
        };
        s.pos = new_pos;
        Ok(Value::int(new_pos as i64))
    } else {
        // Write / append mode: track position in write_buf
        let buf_len = if s.is_binary {
            s.write_buf_bytes.len()
        } else {
            s.write_buf.len()
        };
        let new_pos = match whence {
            0 => {
                if offset < 0 {
                    return Err(PyError::named(
                        "ValueError",
                        format!("negative seek position {offset}"),
                    ));
                }
                (offset as usize).min(buf_len)
            }
            1 => {
                if offset != 0 {
                    return Err(PyError::named(
                        "io.UnsupportedOperation",
                        "can't do nonzero cur-relative seeks".to_string(),
                    ));
                }
                buf_len
            }
            2 => {
                if offset != 0 {
                    return Err(PyError::named(
                        "io.UnsupportedOperation",
                        "can't do nonzero end-relative seeks".to_string(),
                    ));
                }
                buf_len
            }
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    format!("invalid whence ({whence}, should be 0, 1 or 2)"),
                ));
            }
        };
        // Truncate write buffer to new_pos (seek in write mode discards future bytes)
        if s.is_binary {
            s.write_buf_bytes.truncate(new_pos);
        } else {
            s.write_buf.truncate(new_pos);
        }
        Ok(Value::int(new_pos as i64))
    }
}

fn file_read(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let n = args.first().and_then(|v| match v.kind() {
        ValueKind::Int(n) => Some(n),
        _ => None,
    });
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::Runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Err(PyError::named(
            "ValueError",
            "I/O operation on closed file.".to_string(),
        ));
    }
    if !s.is_readable() {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not readable".to_string(),
        ));
    }
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
    let remaining = &bytes[s.pos..];
    let to_take = match n {
        Some(n) if n >= 0 => remaining.len().min(n as usize),
        _ => remaining.len(),
    };
    let mut take = 0usize;
    let mut chars = 0usize;
    let want_chars = match n {
        Some(n) if n >= 0 => n as usize,
        _ => usize::MAX,
    };
    for (i, _) in s.content[s.pos..].char_indices() {
        if chars >= want_chars {
            take = i;
            break;
        }
        chars += 1;
        take = i + s.content[s.pos + i..].chars().next().unwrap().len_utf8();
        if chars >= want_chars {
            break;
        }
    }
    let take = if n.is_some_and(|n| n >= 0) {
        take
    } else {
        to_take
    };
    let out = s.content[s.pos..s.pos + take].to_string();
    s.pos += take;
    Ok(Value::string(out))
}

fn file_write(state: &BuiltinState, args: &[Value]) -> Result<Value> {
    let mut borrow = state.borrow_mut();
    let st = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::Runtime("internal: bad file state".to_string()))?;
    if st.closed {
        return Err(PyError::named(
            "ValueError",
            "I/O operation on closed file.".to_string(),
        ));
    }
    if !st.is_write && !st.is_append {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not writable".to_string(),
        ));
    }
    if st.is_binary {
        // Binary mode: accept bytes, reject str
        match args.first().map(|v| v.kind()) {
            Some(ValueKind::Bytes(rc)) => {
                let data = rc.as_slice().to_vec();
                let len = data.len() as i64;
                st.write_buf_bytes.extend_from_slice(&data);
                Ok(Value::int(len))
            }
            _ => Err(PyError::named(
                "TypeError",
                "a bytes-like object is required, not 'str'".to_string(),
            )),
        }
    } else {
        // Text mode: accept str, reject bytes
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
                let len = s.chars().count() as i64;
                st.write_buf.push_str(&s);
                Ok(Value::int(len))
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
        .ok_or_else(|| PyError::Runtime("internal: bad file state".to_string()))?;
    if st.closed {
        return Err(PyError::named(
            "ValueError",
            "I/O operation on closed file.".to_string(),
        ));
    }
    if st.is_binary {
        for v in lines {
            match v.kind() {
                ValueKind::Bytes(rc) => st.write_buf_bytes.extend_from_slice(rc.as_slice()),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "a bytes-like object is required, not 'str'".to_string(),
                    ));
                }
            }
        }
    } else {
        for v in lines {
            match v.kind() {
                ValueKind::Str(s) => st.write_buf.push_str(s),
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

/// Read the next line from a file and return it as the appropriate Value type
/// (`bytes` in binary mode, `str` in text mode).  Returns `None` at EOF.
fn read_line_value(state: &BuiltinState) -> Result<Option<Value>> {
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::Runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Err(PyError::named(
            "ValueError",
            "I/O operation on closed file.".to_string(),
        ));
    }
    if !s.is_readable() {
        return Err(PyError::named(
            "io.UnsupportedOperation",
            "not readable".to_string(),
        ));
    }
    if s.is_binary {
        let bytes = &s.content_bytes;
        if s.pos >= bytes.len() {
            return Ok(None);
        }
        let mut end = s.pos;
        while end < bytes.len() && bytes[end] != b'\n' {
            end += 1;
        }
        if end < bytes.len() {
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
        let mut end = s.pos;
        while end < bytes.len() && bytes[end] != b'\n' {
            end += 1;
        }
        if end < bytes.len() {
            end += 1;
        }
        let line = s.content[s.pos..end].to_string();
        s.pos = end;
        Ok(Some(Value::string(line)))
    }
}

fn close_file(state: &BuiltinState) -> Result<()> {
    let mut borrow = state.borrow_mut();
    let s = borrow
        .downcast_mut::<FileState>()
        .ok_or_else(|| PyError::Runtime("internal: bad file state".to_string()))?;
    if s.closed {
        return Ok(());
    }
    if s.is_write {
        if s.is_binary {
            std::fs::write(&s.path, &s.write_buf_bytes)
                .map_err(|e| PyError::from_io_error(&e, Some(&s.path)))?;
        } else {
            let enc = s.encoding.as_deref().unwrap_or("utf-8");
            // On Windows, text mode translates \n -> \r\n on write.
            #[cfg(windows)]
            let text_to_encode = s.write_buf.replace('\n', "\r\n");
            #[cfg(not(windows))]
            let text_to_encode: &str = &s.write_buf;
            // On Windows `text_to_encode` is an owned `String`, so the borrow is
            // required there; on other platforms it is already a `&str`.
            #[cfg_attr(not(windows), allow(clippy::needless_borrow))]
            let text_bytes = encode_text_to_bytes(&text_to_encode, enc)?;
            std::fs::write(&s.path, &text_bytes)
                .map_err(|e| PyError::from_io_error(&e, Some(&s.path)))?;
        }
    } else if s.is_append {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&s.path)
            .map_err(|e| PyError::from_io_error(&e, Some(&s.path)))?;
        if s.is_binary {
            f.write_all(&s.write_buf_bytes)
                .map_err(|e| PyError::from_io_error(&e, Some(&s.path)))?;
        } else {
            let enc = s.encoding.as_deref().unwrap_or("utf-8");
            #[cfg(windows)]
            let text_to_encode = s.write_buf.replace('\n', "\r\n");
            #[cfg(not(windows))]
            let text_to_encode: &str = &s.write_buf;
            // On Windows `text_to_encode` is an owned `String`, so the borrow is
            // required there; on other platforms it is already a `&str`.
            #[cfg_attr(not(windows), allow(clippy::needless_borrow))]
            let text_bytes = encode_text_to_bytes(&text_to_encode, enc)?;
            f.write_all(&text_bytes)
                .map_err(|e| PyError::from_io_error(&e, Some(&s.path)))?;
        }
    }
    s.closed = true;
    s.write_buf.clear();
    s.write_buf_bytes.clear();
    Ok(())
}

/// Returns true if `name` is a method on a file object.  Used by `hasattr`.
pub fn has_method(name: &str) -> bool {
    matches!(
        name,
        "__enter__"
            | "__exit__"
            | "__iter__"
            | "__next__"
            | "close"
            | "flush"
            | "read"
            | "readline"
            | "readlines"
            | "seek"
            | "tell"
            | "write"
            | "writelines"
    )
}
