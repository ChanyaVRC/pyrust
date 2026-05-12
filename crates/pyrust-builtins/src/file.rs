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

#[allow(dead_code)] // `mode` is kept for diagnostics / future repr() support
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

    fn repr(&self, _state: &BuiltinState) -> String {
        "<file object>".to_string()
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

    let mut content = String::new();
    if is_read {
        content = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                PyError::named(
                    "FileNotFoundError",
                    format!("[Errno 2] No such file or directory: '{path}'"),
                )
            } else {
                PyError::named("OSError", e.to_string())
            }
        })?;
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
                    "I/O operation on closed file".to_string(),
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
            let len = s.len();
            let mut borrow = state.borrow_mut();
            let st = borrow
                .downcast_mut::<FileState>()
                .ok_or_else(|| PyError::Runtime("internal: bad file state".to_string()))?;
            if st.closed {
                return Err(PyError::named(
                    "ValueError",
                    "I/O operation on closed file".to_string(),
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
                Some(ValueKind::List(items)) | Some(ValueKind::Tuple(items)) => items.to_vec(),
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
                    "I/O operation on closed file".to_string(),
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
            "I/O operation on closed file".to_string(),
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
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
    } else if s.is_append {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&s.path)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        f.write_all(s.write_buf.as_bytes())
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
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
            | "read"
            | "readline"
            | "readlines"
            | "write"
            | "writelines"
    )
}
