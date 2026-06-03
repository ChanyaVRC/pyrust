// `io` module — `io.StringIO` and `io.BytesIO` in-memory stream classes.
//
// Both classes mirror the CPython `io` module's text / bytes in-memory
// buffer interfaces.  State lives in Python-level instance attributes:
//
// - `_buf`    (str / bytes) — the full buffer contents.
// - `_pos`    (int)         — current read/write cursor (byte offset for
//                            bytes; character offset for text).
// - `_closed` (bool)        — True once `close()` has been called.
//
// Instance attrs are updated atomically (no partial writes) so a method
// that reads then writes always sees a consistent snapshot.
//
// ### seek(pos[, whence]) implementation
//
// CPython's StringIO supports only a restricted set of whence values:
//   - whence=0 (SEEK_SET): absolute position — any non-negative offset.
//   - whence=1 (SEEK_CUR): only offset=0 is allowed (returns current pos).
//   - whence=2 (SEEK_END): only offset=0 is allowed (jumps to end).
//
// BytesIO is more permissive: all three whence modes work with any offset.
//
// Reference: <https://docs.python.org/3/library/io.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::{PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;

// ── sentinel error ────────────────────────────────────────────────────────────

fn closed_error() -> PyError {
    PyError::named("ValueError", "I/O operation on closed file".to_string())
}

// ── common self helpers ───────────────────────────────────────────────────────

fn expect_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(&rc)),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() self must be a PyInstance",
        ))),
    }
}

fn is_closed(inst: &Rc<RefCell<PyInstance>>) -> bool {
    matches!(
        inst.borrow().attrs.get("_closed").map(|v| v.kind()),
        Some(ValueKind::Bool(true))
    )
}

fn get_pos(inst: &Rc<RefCell<PyInstance>>) -> i64 {
    match inst.borrow().attrs.get("_pos").map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        _ => 0,
    }
}

fn set_pos(inst: &Rc<RefCell<PyInstance>>, pos: i64) {
    inst.borrow_mut()
        .attrs
        .insert("_pos".to_string(), Value::int(pos));
}

// ── shared method prologue helpers ─────────────────────────────────────────────

/// Resolve `self` and reject operations on a closed stream — the prologue
/// shared by every read/write/seek-style method.
fn open_self(
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Rc<RefCell<PyInstance>>> {
    let inst = expect_self(args, fn_name)?;
    if is_closed(&inst) {
        return Err(closed_error());
    }
    Ok(inst)
}

/// Slice off `self` and enforce the "takes at most 1 argument" arity used by
/// `read` / `readline` / `readlines` / `truncate`.
fn user_at_most_one<'a>(
    args: &'a [ExpandedCallArg],
    fn_name: &str,
) -> Result<&'a [ExpandedCallArg]> {
    let user = &args[1..];
    if user.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes at most 1 argument"),
        ));
    }
    Ok(user)
}

/// Parse the optional `size` argument of `read` / `readline`: `None`,
/// negative, or omitted means "no limit"; bool/int give the limit.
fn parse_optional_size(
    user: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Option<usize>> {
    match user.first().map(|a| a.value.kind()) {
        None | Some(ValueKind::None) => Ok(None),
        Some(ValueKind::Int(n)) if n < 0 => Ok(None),
        Some(ValueKind::Int(n)) => Ok(Some(n as usize)),
        Some(ValueKind::Bool(b)) => Ok(Some(b as usize)),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() size must be an integer"),
        )),
    }
}

// ── StringIO ─────────────────────────────────────────────────────────────────

pyrust_module! {
    class StringIO {
        /// `StringIO(initial_value='', newline='\n')` — creates an in-memory
        /// text stream initialised to `initial_value`.  The `newline`
        /// parameter is accepted but ignored (we store the string as-is).
        /// <https://docs.python.org/3/library/io.html#io.StringIO>
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            // Accept at most 2 user args: initial_value and newline.
            if user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes at most 2 arguments ({} given)", user.len()),
                ));
            }
            let initial = match user.first() {
                Some(a) => match a.value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() initial_value must be str, not {}",
                            pyrust_core::builtin_type_name(&a.value)
                        ),
                    )),
                },
                None => String::new(),
            };
            // newline must be None or str (value is accepted-and-ignored).
            if let Some(nl) = user.get(1) {
                match nl.value.kind() {
                    ValueKind::None | ValueKind::Str(_) => {}
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() newline must be str or None, not {}",
                            pyrust_core::builtin_type_name(&nl.value)
                        ),
                    )),
                }
            }
            let _ = _interp;
            let mut attrs = inst.borrow_mut();
            attrs.attrs.insert("_buf".to_string(), Value::string(&initial));
            attrs.attrs.insert("_pos".to_string(), Value::int(0));
            attrs.attrs.insert("_closed".to_string(), Value::bool_(false));
            Ok(Value::none())
        }

        /// `read([size=-1])` — read up to `size` characters from the current
        /// position.  If `size` is negative or omitted, read to end.
        fn read(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let size = parse_optional_size(user, FN_NAME)?;
            let _ = _interp;
            let buf = string_io_buf(&inst, FN_NAME)?;
            let pos = get_pos(&inst) as usize;
            let chars: Vec<char> = buf.chars().collect();
            let total = chars.len();
            let start = pos.min(total);
            let end = match size {
                None => total,
                Some(n) => (start + n).min(total),
            };
            let result: String = chars[start..end].iter().collect();
            set_pos(&inst, end as i64);
            Ok(Value::string(result))
        }

        /// `readline([size=-1])` — read up to the next newline (inclusive)
        /// or `size` characters, whichever comes first.
        fn readline(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let size_limit = parse_optional_size(user, FN_NAME)?;
            let _ = _interp;
            let buf = string_io_buf(&inst, FN_NAME)?;
            let pos = get_pos(&inst) as usize;
            let chars: Vec<char> = buf.chars().collect();
            let total = chars.len();
            let start = pos.min(total);
            let max_read = size_limit.unwrap_or(total - start);
            let mut count = 0;
            for ch in &chars[start..] {
                count += 1;
                if count > max_read { count -= 1; break; }
                if *ch == '\n' { break; }
            }
            let result: String = chars[start..start + count].iter().collect();
            set_pos(&inst, (start + count) as i64);
            Ok(Value::string(result))
        }

        /// `readlines([hint])` — read all remaining lines into a list.
        fn readlines(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            user_at_most_one(args, FN_NAME)?;
            let _ = _interp;
            let buf = string_io_buf(&inst, FN_NAME)?;
            let pos = get_pos(&inst) as usize;
            let chars: Vec<char> = buf.chars().collect();
            let total = chars.len();
            let start = pos.min(total);
            let remaining: String = chars[start..].iter().collect();
            set_pos(&inst, total as i64);
            let lines: Vec<Value> = if remaining.is_empty() {
                Vec::new()
            } else {
                let mut out = Vec::new();
                let mut cur = String::new();
                for ch in remaining.chars() {
                    cur.push(ch);
                    if ch == '\n' {
                        out.push(Value::string(&cur));
                        cur.clear();
                    }
                }
                if !cur.is_empty() {
                    out.push(Value::string(&cur));
                }
                out
            };
            Ok(Value::list(lines))
        }

        /// `write(s)` — write `s` at the current position, extending the
        /// buffer if needed.  Returns the number of characters written.
        fn write(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let s = match args[1].value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument must be str"),
                )),
            };
            let _ = _interp;
            let buf = string_io_buf(&inst, FN_NAME)?;
            let pos = get_pos(&inst) as usize;
            let chars: Vec<char> = buf.chars().collect();
            let written: Vec<char> = s.chars().collect();
            let n_written = written.len();
            let total = chars.len();
            // Splice: [0..pos] + written + [pos+n_written..total]
            let mut new_chars: Vec<char> = Vec::with_capacity(total.max(pos + n_written));
            // Fill gap between pos and current end with NUL if pos > total
            if pos > total {
                new_chars.extend_from_slice(&chars[..]);
                new_chars.extend(std::iter::repeat('\0').take(pos - total));
            } else {
                new_chars.extend_from_slice(&chars[..pos]);
            }
            new_chars.extend_from_slice(&written);
            let end = (pos + n_written).min(total);
            if end < total {
                new_chars.extend_from_slice(&chars[end..]);
            }
            let new_buf: String = new_chars.into_iter().collect();
            inst.borrow_mut()
                .attrs
                .insert("_buf".to_string(), Value::string(&new_buf));
            set_pos(&inst, (pos + n_written) as i64);
            Ok(Value::int(n_written as i64))
        }

        /// `getvalue()` — return the entire buffer contents regardless of
        /// current position.  `ValueError` if the stream is closed.
        fn getvalue(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let _ = _interp;
            let buf = string_io_buf(&inst, FN_NAME)?;
            Ok(Value::string(buf))
        }

        /// `seek(pos[, whence=0])` — set the stream position.
        /// StringIO only supports whence=0 (absolute) and whence=2 with
        /// offset=0 (jump to end) and whence=1 with offset=0 (no-op /
        /// current pos) — mirroring CPython's restrictions.
        fn seek(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.is_empty() || user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes 1 or 2 arguments"),
                ));
            }
            let offset = match user[0].value.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() offset must be an integer"),
                )),
            };
            let whence = match user.get(1).map(|a| a.value.kind()) {
                None => 0i64,
                Some(ValueKind::Int(n)) => n,
                Some(ValueKind::Bool(b)) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() whence must be an integer"),
                )),
            };
            let _ = _interp;
            let buf = string_io_buf(&inst, FN_NAME)?;
            let char_len = buf.chars().count() as i64;
            let new_pos = match whence {
                0 => {
                    // SEEK_SET — absolute
                    if offset < 0 {
                        return Err(PyError::named(
                            "ValueError",
                            format!("{FN_NAME}(): negative seek position {offset}"),
                        ));
                    }
                    offset
                }
                1 => {
                    // SEEK_CUR — StringIO only allows offset=0
                    if offset != 0 {
                        return Err(PyError::named(
                            "OSError",
                            "Can't do nonzero cur-relative seeks".to_string(),
                        ));
                    }
                    get_pos(&inst)
                }
                2 => {
                    // SEEK_END — StringIO only allows offset=0
                    if offset != 0 {
                        return Err(PyError::named(
                            "OSError",
                            "Can't do nonzero end-relative seeks".to_string(),
                        ));
                    }
                    char_len
                }
                _ => return Err(PyError::named(
                    "ValueError",
                    format!("{FN_NAME}(): unsupported whence value {whence}"),
                )),
            };
            let clamped = new_pos.max(0);
            set_pos(&inst, clamped);
            Ok(Value::int(clamped))
        }

        /// `tell()` — return the current stream position (character offset).
        fn tell(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let _ = _interp;
            Ok(Value::int(get_pos(&inst)))
        }

        /// `truncate([size=None])` — truncate the buffer to at most `size`
        /// characters.  The current position is unchanged.  Returns the
        /// new size.
        fn truncate(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let _ = _interp;
            let buf = string_io_buf(&inst, FN_NAME)?;
            let chars: Vec<char> = buf.chars().collect();
            let total = chars.len() as i64;
            let size = match user.first().map(|a| a.value.kind()) {
                None | Some(ValueKind::None) => get_pos(&inst),
                Some(ValueKind::Int(n)) if n >= 0 => n,
                Some(ValueKind::Bool(b)) => b as i64,
                Some(ValueKind::Int(_)) => return Err(PyError::named(
                    "ValueError",
                    format!("{FN_NAME}(): negative size"),
                )),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}(): size must be an integer"),
                )),
            };
            let new_len = size.min(total).max(0) as usize;
            let new_buf: String = chars[..new_len].iter().collect();
            inst.borrow_mut()
                .attrs
                .insert("_buf".to_string(), Value::string(&new_buf));
            Ok(Value::int(new_len as i64))
        }

        /// `close()` — mark the stream as closed.  Subsequent reads/writes
        /// will raise `ValueError`.
        fn close(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let _ = _interp;
            inst.borrow_mut()
                .attrs
                .insert("_closed".to_string(), Value::bool_(true));
            Ok(Value::none())
        }

        /// `__enter__` — context manager support; returns `self`.
        fn __enter__(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let _ = _interp;
            Ok(Value::py_instance(inst))
        }

        /// `__exit__` — call `close()`.
        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            inst.borrow_mut()
                .attrs
                .insert("_closed".to_string(), Value::bool_(true));
            Ok(Value::bool_(false))
        }

        fn __repr__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let closed = is_closed(&inst);
            let _ = _interp;
            if closed {
                Ok(Value::string("<_io.StringIO [closed]>".to_string()))
            } else {
                Ok(Value::string("<_io.StringIO object>".to_string()))
            }
        }
    }

    // ── BytesIO ───────────────────────────────────────────────────────────────

    class BytesIO {
        /// `BytesIO(initial_bytes=b'')` — creates an in-memory bytes stream.
        /// <https://docs.python.org/3/library/io.html#io.BytesIO>
        fn __init__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes at most 1 argument ({} given)", user.len()),
                ));
            }
            let initial: Vec<u8> = match user.first() {
                Some(a) => match a.value.kind() {
                    ValueKind::Bytes(b) => b.to_vec(),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() initial_bytes must be a bytes-like object, not {}",
                            pyrust_core::builtin_type_name(&a.value)
                        ),
                    )),
                },
                None => Vec::new(),
            };
            let _ = _interp;
            let mut attrs = inst.borrow_mut();
            attrs.attrs.insert("_buf".to_string(), Value::bytes(initial));
            attrs.attrs.insert("_pos".to_string(), Value::int(0));
            attrs.attrs.insert("_closed".to_string(), Value::bool_(false));
            Ok(Value::none())
        }

        /// `read([size=-1])` — read up to `size` bytes.
        fn read(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let size = parse_optional_size(user, FN_NAME)?;
            let _ = _interp;
            let buf = bytes_io_buf(&inst, FN_NAME)?;
            let pos = get_pos(&inst) as usize;
            let total = buf.len();
            let start = pos.min(total);
            let end = match size {
                None => total,
                Some(n) => (start + n).min(total),
            };
            let result = buf[start..end].to_vec();
            set_pos(&inst, end as i64);
            Ok(Value::bytes(result))
        }

        /// `readline([size=-1])` — read up to the next `\n` byte (inclusive).
        fn readline(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let size_limit = parse_optional_size(user, FN_NAME)?;
            let _ = _interp;
            let buf = bytes_io_buf(&inst, FN_NAME)?;
            let pos = get_pos(&inst) as usize;
            let total = buf.len();
            let start = pos.min(total);
            let max_read = size_limit.unwrap_or(total - start);
            let mut count = 0;
            for &b in &buf[start..] {
                if count >= max_read { break; }
                count += 1;
                if b == b'\n' { break; }
            }
            let result = buf[start..start + count].to_vec();
            set_pos(&inst, (start + count) as i64);
            Ok(Value::bytes(result))
        }

        /// `readlines([hint])` — read all remaining lines into a list of bytes.
        fn readlines(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            user_at_most_one(args, FN_NAME)?;
            let _ = _interp;
            let buf = bytes_io_buf(&inst, FN_NAME)?;
            let pos = get_pos(&inst) as usize;
            let total = buf.len();
            let start = pos.min(total);
            let remaining = &buf[start..];
            set_pos(&inst, total as i64);
            let mut lines = Vec::new();
            let mut cur_start = 0;
            for (i, &b) in remaining.iter().enumerate() {
                if b == b'\n' {
                    lines.push(Value::bytes(remaining[cur_start..=i].to_vec()));
                    cur_start = i + 1;
                }
            }
            if cur_start < remaining.len() {
                lines.push(Value::bytes(remaining[cur_start..].to_vec()));
            }
            Ok(Value::list(lines))
        }

        /// `write(b)` — write bytes at the current position.
        fn write(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let data: Vec<u8> = match args[1].value.kind() {
                ValueKind::Bytes(b) => b.to_vec(),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument must be bytes-like object"),
                )),
            };
            let _ = _interp;
            let buf = bytes_io_buf(&inst, FN_NAME)?;
            let pos = get_pos(&inst) as usize;
            let n_written = data.len();
            let total = buf.len();
            let mut new_buf: Vec<u8> = Vec::with_capacity(total.max(pos + n_written));
            if pos > total {
                new_buf.extend_from_slice(&buf[..]);
                new_buf.extend(std::iter::repeat(0u8).take(pos - total));
            } else {
                new_buf.extend_from_slice(&buf[..pos]);
            }
            new_buf.extend_from_slice(&data);
            let end = (pos + n_written).min(total);
            if end < total {
                new_buf.extend_from_slice(&buf[end..]);
            }
            inst.borrow_mut()
                .attrs
                .insert("_buf".to_string(), Value::bytes(new_buf));
            set_pos(&inst, (pos + n_written) as i64);
            Ok(Value::int(n_written as i64))
        }

        /// `getvalue()` — return the full buffer regardless of position.
        fn getvalue(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let _ = _interp;
            let buf = bytes_io_buf(&inst, FN_NAME)?;
            Ok(Value::bytes(buf))
        }

        /// `seek(pos[, whence=0])` — BytesIO supports all three whence modes
        /// with arbitrary offsets (unlike StringIO which restricts them).
        fn seek(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let user = &args[1..];
            if user.is_empty() || user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes 1 or 2 arguments"),
                ));
            }
            let offset = match user[0].value.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() offset must be an integer"),
                )),
            };
            let whence = match user.get(1).map(|a| a.value.kind()) {
                None => 0i64,
                Some(ValueKind::Int(n)) => n,
                Some(ValueKind::Bool(b)) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() whence must be an integer"),
                )),
            };
            let _ = _interp;
            let buf = bytes_io_buf(&inst, FN_NAME)?;
            let buf_len = buf.len() as i64;
            let current = get_pos(&inst);
            let new_pos = match whence {
                0 => offset, // SEEK_SET
                1 => current + offset, // SEEK_CUR
                2 => buf_len + offset, // SEEK_END
                _ => return Err(PyError::named(
                    "ValueError",
                    format!("{FN_NAME}(): invalid whence value {whence}"),
                )),
            };
            if new_pos < 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("{FN_NAME}(): negative seek position {new_pos}"),
                ));
            }
            set_pos(&inst, new_pos);
            Ok(Value::int(new_pos))
        }

        /// `tell()` — return the current position (byte offset).
        fn tell(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let _ = _interp;
            Ok(Value::int(get_pos(&inst)))
        }

        /// `truncate([size=None])` — truncate to at most `size` bytes.
        fn truncate(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let _ = _interp;
            let buf = bytes_io_buf(&inst, FN_NAME)?;
            let total = buf.len() as i64;
            let size = match user.first().map(|a| a.value.kind()) {
                None | Some(ValueKind::None) => get_pos(&inst),
                Some(ValueKind::Int(n)) if n >= 0 => n,
                Some(ValueKind::Bool(b)) => b as i64,
                Some(ValueKind::Int(_)) => return Err(PyError::named(
                    "ValueError",
                    format!("{FN_NAME}(): negative size"),
                )),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}(): size must be an integer"),
                )),
            };
            let new_len = size.min(total).max(0) as usize;
            let new_buf = buf[..new_len].to_vec();
            inst.borrow_mut()
                .attrs
                .insert("_buf".to_string(), Value::bytes(new_buf));
            Ok(Value::int(new_len as i64))
        }

        /// `close()` — mark the stream as closed.
        fn close(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let _ = _interp;
            inst.borrow_mut()
                .attrs
                .insert("_closed".to_string(), Value::bool_(true));
            Ok(Value::none())
        }

        fn __enter__(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let _ = _interp;
            Ok(Value::py_instance(inst))
        }

        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            inst.borrow_mut()
                .attrs
                .insert("_closed".to_string(), Value::bool_(true));
            Ok(Value::bool_(false))
        }

        fn __repr__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let closed = is_closed(&inst);
            let _ = _interp;
            if closed {
                Ok(Value::string("<_io.BytesIO [closed]>".to_string()))
            } else {
                Ok(Value::string("<_io.BytesIO object>".to_string()))
            }
        }
    }
}

// ── buffer accessors ──────────────────────────────────────────────────────────

fn string_io_buf(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<String> {
    match inst.borrow().attrs.get("_buf").map(|v| v.kind()) {
        Some(ValueKind::Str(s)) => Ok(s.to_string()),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() StringIO._buf corrupted",
        ))),
    }
}

fn bytes_io_buf(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<Vec<u8>> {
    match inst.borrow().attrs.get("_buf").map(|v| v.kind()) {
        Some(ValueKind::Bytes(b)) => Ok(b.to_vec()),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() BytesIO._buf corrupted",
        ))),
    }
}
