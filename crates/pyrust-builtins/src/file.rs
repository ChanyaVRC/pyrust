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

pub struct FileState {
    pub path: String,
    pub mode: String,
    pub closed: bool,
    /// For read modes: the full file contents; we read eagerly at open time.
    pub content: String,
    /// Read cursor into `content` (in bytes).
    pub pos: usize,
    /// For write modes: buffered bytes to flush on `close()`.
    pub write_buf: String,
    pub is_write: bool,
    pub is_append: bool,
    /// Whether the file was opened in binary mode (`'b'` in mode string).
    pub is_binary: bool,
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
                format!(
                    "<_io.TextIOWrapper name='{}' mode='{}' encoding='UTF-8'>",
                    s.path, s.mode
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
        Ok(read_line_or_none(state)?.map(Value::string))
    }

    fn is_iterable(&self) -> bool {
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
                    Some(Value::string("UTF-8"))
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

/// Implements `open(path, mode='r')`.  Returns a `BuiltinObject` carrying
/// the file state plus the static `FileOps` dispatch table.
pub fn open(path: &str, mode: &str) -> Result<Value> {
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

    let mut content = String::new();
    if is_read {
        content =
            std::fs::read_to_string(path).map_err(|e| PyError::from_io_error(&e, Some(path)))?;
    }
    let state = FileState {
        path: path.to_string(),
        mode: mode.to_string(),
        closed: false,
        content,
        pos: 0,
        write_buf: String::new(),
        is_write,
        is_append,
        is_binary,
    };
    let state: Box<dyn Any> = Box::new(state);
    Ok(Value::builtin_object(FILE_OPS, state))
}

fn call_file_method_inner(state: &BuiltinState, method: &str, args: &[Value]) -> Result<Value> {
    match method {
        "__enter__" => Ok(Value::builtin_object_shared(FILE_OPS, Rc::clone(state))),
        "__exit__" => {
            close_file(state)?;
            Ok(Value::none())
        }
        "__iter__" => Ok(Value::builtin_object_shared(FILE_OPS, Rc::clone(state))),
        "__next__" => {
            let line = read_line_or_none(state)?;
            match line {
                Some(s) => Ok(Value::string(s)),
                None => Err(PyError::named("StopIteration", String::new())),
            }
        }
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
            } else {
                s.write_buf.len()
            };
            Ok(Value::int(pos as i64))
        }
        "seek" => {
            let offset = args
                .first()
                .and_then(|v| match v.kind() {
                    ValueKind::Int(n) => Some(n),
                    _ => None,
                })
                .ok_or_else(|| {
                    PyError::named("TypeError", "seek() argument must be int".to_string())
                })?;
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
                let content_len = s.content.len();
                let new_pos = match whence {
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
                };
                s.pos = new_pos;
                Ok(Value::int(new_pos as i64))
            } else {
                // Write / append mode: track position in write_buf
                let buf_len = s.write_buf.len();
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
                // Truncate write_buf to new_pos (seek in write mode discards future bytes)
                s.write_buf.truncate(new_pos);
                Ok(Value::int(new_pos as i64))
            }
        }
        "read" => {
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
        "readline" => {
            let line = read_line_or_none(state)?;
            Ok(Value::string(line.unwrap_or_default()))
        }
        "readlines" => {
            let mut out = Vec::new();
            while let Some(line) = read_line_or_none(state)? {
                out.push(Value::string(line));
            }
            Ok(Value::list(out))
        }
        "write" => {
            let s = args
                .first()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .ok_or_else(|| {
                    PyError::named("TypeError", "write() argument must be str".to_string())
                })?;
            let len = s.chars().count();
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
            st.write_buf.push_str(&s);
            Ok(Value::int(len as i64))
        }
        "writelines" => {
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
            Ok(Value::none())
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'{TYPE_NAME}' object has no attribute '{method}'"),
        )),
    }
}

fn read_line_or_none(state: &BuiltinState) -> Result<Option<String>> {
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
    Ok(Some(line))
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
        std::fs::write(&s.path, &s.write_buf)
            .map_err(|e| PyError::from_io_error(&e, Some(&s.path)))?;
    } else if s.is_append {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&s.path)
            .map_err(|e| PyError::from_io_error(&e, Some(&s.path)))?;
        f.write_all(s.write_buf.as_bytes())
            .map_err(|e| PyError::from_io_error(&e, Some(&s.path)))?;
    }
    s.closed = true;
    s.write_buf.clear();
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

// ── Standard stream wrappers (sys.stdin / sys.stdout / sys.stderr) ────────────

/// Which standard I/O channel this object wraps.
#[derive(Clone, Copy, Debug)]
pub enum StdioKind {
    Stdin,
    Stdout,
    Stderr,
}

/// State carried in the `BuiltinObject` for a stdio stream.
pub struct StdioState {
    pub kind: StdioKind,
}

pub struct StdioOps;

pub const STDIO_OPS: &StdioOps = &StdioOps;
pub const STDIO_TYPE_NAME: &str = "_io.TextIOWrapper";

impl BuiltinTypeOps for StdioOps {
    fn type_name(&self) -> &'static str {
        STDIO_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<StdioState>()
            .expect("StdioOps: bad state");
        match s.kind {
            StdioKind::Stdin => "<_io.TextIOWrapper name='<stdin>' mode='r' encoding='utf-8'>",
            StdioKind::Stdout => "<_io.TextIOWrapper name='<stdout>' mode='w' encoding='utf-8'>",
            StdioKind::Stderr => "<_io.TextIOWrapper name='<stderr>' mode='w' encoding='utf-8'>",
        }
        .to_string()
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn has_method(&self, name: &str) -> bool {
        stdio_has_method(name)
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        name: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        call_stdio_method(state, name, &args)
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<StdioState>()
            .expect("StdioOps: bad state");
        match name {
            "name" => Some(Value::string(match s.kind {
                StdioKind::Stdin => "<stdin>",
                StdioKind::Stdout => "<stdout>",
                StdioKind::Stderr => "<stderr>",
            })),
            "mode" => Some(Value::string(match s.kind {
                StdioKind::Stdin => "r",
                StdioKind::Stdout | StdioKind::Stderr => "w",
            })),
            "encoding" => Some(Value::string("utf-8")),
            "closed" => Some(Value::bool_(false)),
            _ => None,
        }
    }
}

/// Create a `sys.stdout` value.
pub fn make_stdout() -> Value {
    Value::builtin_object(
        STDIO_OPS,
        Box::new(StdioState {
            kind: StdioKind::Stdout,
        }),
    )
}

/// Create a `sys.stderr` value.
pub fn make_stderr() -> Value {
    Value::builtin_object(
        STDIO_OPS,
        Box::new(StdioState {
            kind: StdioKind::Stderr,
        }),
    )
}

/// Create a `sys.stdin` value.
pub fn make_stdin() -> Value {
    Value::builtin_object(
        STDIO_OPS,
        Box::new(StdioState {
            kind: StdioKind::Stdin,
        }),
    )
}

pub fn stdio_has_method(name: &str) -> bool {
    matches!(
        name,
        "write" | "flush" | "fileno" | "read" | "readline" | "readlines"
    )
}

fn call_stdio_method(state: &BuiltinState, method: &str, args: &[Value]) -> Result<Value> {
    let kind = {
        let borrow = state.borrow();
        borrow
            .downcast_ref::<StdioState>()
            .expect("StdioOps: bad state")
            .kind
    };
    match method {
        "write" => {
            let first = args.first();
            let s = first
                .and_then(|v| match v.kind() {
                    ValueKind::Str(s) => Some(s.to_string()),
                    _ => None,
                })
                .ok_or_else(|| {
                    // CPython's C implementation uses tp_name, which for NoneType
                    // is "NoneType" yet the error prints "None".  Mirror that by
                    // mapping NoneType→None in the error message.
                    let type_name = first
                        .map(|v| match v.kind() {
                            ValueKind::None => "None".to_string(),
                            _ => pyrust_core::builtin_type_name(v).into_owned(),
                        })
                        .unwrap_or_else(|| "str".to_string());
                    PyError::named(
                        "TypeError",
                        format!("write() argument must be str, not {type_name}"),
                    )
                })?;
            let n = s.chars().count() as i64;
            match kind {
                StdioKind::Stdout => {
                    use std::io::Write;
                    print!("{s}");
                    let _ = std::io::stdout().flush();
                }
                StdioKind::Stderr => {
                    use std::io::Write;
                    eprint!("{s}");
                    let _ = std::io::stderr().flush();
                }
                StdioKind::Stdin => {
                    return Err(PyError::named(
                        "io.UnsupportedOperation",
                        "not writable".to_string(),
                    ));
                }
            }
            Ok(Value::int(n))
        }
        "flush" => {
            use std::io::Write;
            match kind {
                StdioKind::Stdout => {
                    let _ = std::io::stdout().flush();
                }
                StdioKind::Stderr => {
                    let _ = std::io::stderr().flush();
                }
                StdioKind::Stdin => {}
            }
            Ok(Value::none())
        }
        "fileno" => {
            let fd = match kind {
                StdioKind::Stdin => 0i64,
                StdioKind::Stdout => 1,
                StdioKind::Stderr => 2,
            };
            Ok(Value::int(fd))
        }
        "read" => {
            if !matches!(kind, StdioKind::Stdin) {
                return Err(PyError::named(
                    "io.UnsupportedOperation",
                    "not readable".to_string(),
                ));
            }
            use std::io::Read;
            let size: Option<usize> = args.first().and_then(|v| match v.kind() {
                ValueKind::Int(n) if n >= 0 => Some(n as usize),
                ValueKind::None => None,
                ValueKind::Int(_) => None,
                _ => None,
            });
            let mut buf = String::new();
            match size {
                None => {
                    std::io::stdin().read_to_string(&mut buf).ok();
                }
                Some(n) => {
                    let mut tmp = vec![0u8; n];
                    let got = std::io::stdin().read(&mut tmp).unwrap_or(0);
                    buf = String::from_utf8_lossy(&tmp[..got]).into_owned();
                }
            }
            Ok(Value::string(buf))
        }
        "readline" => {
            if !matches!(kind, StdioKind::Stdin) {
                return Err(PyError::named(
                    "io.UnsupportedOperation",
                    "not readable".to_string(),
                ));
            }
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok();
            Ok(Value::string(line))
        }
        "readlines" => {
            if !matches!(kind, StdioKind::Stdin) {
                return Err(PyError::named(
                    "io.UnsupportedOperation",
                    "not readable".to_string(),
                ));
            }
            use std::io::BufRead;
            let lines: Vec<Value> = std::io::stdin()
                .lock()
                .lines()
                .filter_map(|l| l.ok())
                .map(|l| Value::string(l + "\n"))
                .collect();
            Ok(Value::list(lines))
        }
        _ => Err(PyError::named(
            "AttributeError",
            format!("'{}' object has no attribute '{method}'", STDIO_TYPE_NAME),
        )),
    }
}
