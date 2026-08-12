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
// This file is the Python registration and argument-validation facade. The
// character-oriented and byte-oriented cursor/buffer policies live in the
// `string_buffer` and `bytes_buffer` child modules respectively.
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
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyBigInt, PyDict, PyInstance, PyKey, PyToPrimitive, Value, ValueKind};
use pyrust_derive::pyrust_module;

#[path = "io/bytes_buffer.rs"]
mod bytes_buffer;
#[path = "io/string_buffer.rs"]
mod string_buffer;

/// Python-source supplements for the `io` module (constants, `open` alias, and
/// the `IOBase` family of abstract base classes).  Exec'd once at first import
/// (see `inject_python_members`).
const IO_PY_SOURCE: &str = include_str!("io_py.py");

/// Public names from `IO_PY_SOURCE` copied onto the `io` module.
const IO_PY_EXPORTS: [&str; 9] = [
    "SEEK_SET",
    "SEEK_CUR",
    "SEEK_END",
    "DEFAULT_BUFFER_SIZE",
    "open",
    "IOBase",
    "RawIOBase",
    "BufferedIOBase",
    "TextIOBase",
];

/// Exec `IO_PY_SOURCE` once, copy its public names onto the `io` module, and
/// re-parent the native `BytesIO` / `StringIO` classes onto `BufferedIOBase` /
/// `TextIOBase` so the CPython isinstance hierarchy holds (issue #2778).
/// Called from the `io` post-load hook in `env.rs::load_module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = Value::dict(PyDict::default());
    interp.exec_source(IO_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("io: exec namespace not a dict".into()))?;
    for name in IO_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            // The ABC classes render their repr / `__module__` via the exec
            // namespace's `__name__`, which defaults to `__main__`; override it
            // so `io.IOBase.__module__ == "io"` matches CPython.
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("io"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    // Re-parent the native concrete stream classes onto the matching ABC so
    // `isinstance(BytesIO(), BufferedIOBase)` / `isinstance(StringIO(),
    // TextIOBase)` (and transitively `IOBase`) hold.  The macro builds both
    // with `base: None`, so this is the first parent assigned.
    for (concrete, abc) in [("BytesIO", "BufferedIOBase"), ("StringIO", "TextIOBase")] {
        let concrete_val = module.borrow().attrs.get(concrete).cloned();
        let abc_val = module.borrow().attrs.get(abc).cloned();
        if let (Some(c), Some(a)) = (concrete_val, abc_val)
            && let (ValueKind::PyClass(c_rc), ValueKind::PyClass(a_rc)) = (c.kind(), a.kind())
        {
            let error_name = match concrete {
                "BytesIO" => "_io.BytesIO",
                "StringIO" => "_io.StringIO",
                _ => unreachable!(),
            };
            let mut class = c_rc.borrow_mut();
            class.error_name = Some(error_name);
            if class.base.is_none() {
                class.base = Some(Rc::clone(a_rc));
                drop(class);
                a_rc.borrow()
                    .subclasses
                    .borrow_mut()
                    .push(Rc::downgrade(c_rc));
            }
        }
    }
    Ok(Some(ns.clone()))
}

// ── sentinel error ────────────────────────────────────────────────────────────

fn closed_error() -> PyError {
    PyError::named("ValueError", "I/O operation on closed file".to_string())
}

/// Closed-file error with the trailing period.  CPython's `_io` module is
/// internally inconsistent about this: `BytesIO` raises every closed-file
/// error with a trailing `.`, and `StringIO.isatty()` / line iteration do
/// too, while `StringIO.read`/`seek`/`tell`/`seekable`/… omit it.  We match
/// each site exactly.
fn closed_error_dot() -> PyError {
    PyError::named("ValueError", "I/O operation on closed file.".to_string())
}

/// `io.UnsupportedOperation` — raised by `fileno()` whether the stream is
/// open or closed (CPython raises it before the closed-state check).
fn unsupported_fileno() -> PyError {
    PyError::named("io.UnsupportedOperation", "fileno".to_string())
}

// ── common self helpers ───────────────────────────────────────────────────────

fn expect_self(args: &[ExpandedCallArg], fn_name: &str) -> Result<Rc<RefCell<PyInstance>>> {
    match args.first().map(|a| a.value.kind()) {
        Some(ValueKind::PyInstance(rc)) => Ok(Rc::clone(rc)),
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
    inst.borrow_mut().attrs.insert("_pos", Value::int(pos));
}

// ── shared method prologue helpers ─────────────────────────────────────────────

/// Resolve `self` and reject operations on a closed stream — the prologue
/// shared by every read/write/seek-style method.
fn open_self(args: &[ExpandedCallArg], fn_name: &str) -> Result<Rc<RefCell<PyInstance>>> {
    let inst = expect_self(args, fn_name)?;
    if is_closed(&inst) {
        return Err(closed_error());
    }
    Ok(inst)
}

/// Like `open_self`, but raises the trailing-period variant of the closed-file
/// error.  CPython's `BytesIO` uses the period on every closed-file message,
/// and `StringIO.__enter__` does too, so those sites use this prologue.
fn open_self_dot(args: &[ExpandedCallArg], fn_name: &str) -> Result<Rc<RefCell<PyInstance>>> {
    let inst = expect_self(args, fn_name)?;
    if is_closed(&inst) {
        return Err(closed_error_dot());
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
fn parse_optional_size(user: &[ExpandedCallArg], fn_name: &str) -> Result<Option<usize>> {
    match user.first().map(|a| a.value.kind()) {
        None | Some(ValueKind::None) => Ok(None),
        Some(ValueKind::Int(n)) if n < 0 => Ok(None),
        // On a narrower host, an oversized read limit is equivalent to an
        // unbounded limit because no backing buffer can reach it.
        Some(ValueKind::Int(n)) => Ok(Some(usize::try_from(n).unwrap_or(usize::MAX))),
        Some(ValueKind::BigInt(n)) if n < &PyBigInt::from(0) => Ok(None),
        Some(ValueKind::BigInt(n)) => Ok(Some(n.to_usize().unwrap_or(usize::MAX))),
        Some(ValueKind::Bool(b)) => Ok(Some(b as usize)),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() size must be an integer"),
        )),
    }
}

fn parse_io_offset(value: &Value, fn_name: &str, arg_name: &str) -> Result<i64> {
    match value.kind() {
        ValueKind::Int(value) => Ok(value),
        ValueKind::Bool(value) => Ok(value as i64),
        ValueKind::BigInt(value) => value.to_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() {arg_name} must be an integer"),
        )),
    }
}

fn parse_io_whence(value: &Value, fn_name: &str) -> Result<i64> {
    let value = match value.kind() {
        ValueKind::Int(value) => i32::try_from(value).ok(),
        ValueKind::Bool(value) => Some(value as i32),
        ValueKind::BigInt(value) => value.to_i32(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() whence must be an integer"),
            ));
        }
    };
    value.map(i64::from).ok_or_else(|| {
        PyError::named(
            "OverflowError",
            "Python int too large to convert to C int".to_string(),
        )
    })
}

fn parse_truncate_size(value: &Value, fn_name: &str) -> Result<i64> {
    match value.kind() {
        ValueKind::Int(value) => Ok(value),
        ValueKind::Bool(value) => Ok(value as i64),
        ValueKind::BigInt(value) => value.to_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "cannot fit 'int' into an index-sized integer".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}(): size must be an integer"),
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
            string_buffer::initialize(&inst, &initial);
            Ok(Value::none())
        }

        /// `read([size=-1])` — read up to `size` characters from the current
        /// position.  If `size` is negative or omitted, read to end.
        fn read(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let size = parse_optional_size(user, FN_NAME)?;
            let _ = _interp;
            Ok(Value::string(string_buffer::read(&inst, size, FN_NAME)?))
        }

        /// `readline([size=-1])` — read up to the next newline (inclusive)
        /// or `size` characters, whichever comes first.
        fn readline(args) -> Result<Value> {
            let inst = open_self(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let size_limit = parse_optional_size(user, FN_NAME)?;
            let _ = _interp;
            Ok(Value::string(string_buffer::read_line(
                &inst, size_limit, FN_NAME,
            )?))
        }

        /// `readlines([hint])` — read all remaining lines into a list.
        /// CPython's StringIO.readlines closed-file error carries the trailing
        /// period (unlike read/readline/tell, which omit it).
        fn readlines(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            user_at_most_one(args, FN_NAME)?;
            let _ = _interp;
            let lines = string_buffer::read_lines(&inst, FN_NAME)?
                .into_iter()
                .map(Value::string)
                .collect();
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
            let n_written = string_buffer::write(&inst, &s)?;
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
            Ok(Value::string(string_buffer::contents(&inst, FN_NAME)?))
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
            let offset = parse_io_offset(&user[0].value, FN_NAME, "offset")?;
            let whence = match user.get(1) {
                None => 0i64,
                Some(arg) => parse_io_whence(&arg.value, FN_NAME)?,
            };
            let _ = _interp;
            Ok(Value::int(string_buffer::seek(
                &inst, offset, whence, FN_NAME,
            )?))
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
            let size = match user.first().map(|a| a.value.kind()) {
                None | Some(ValueKind::None) => get_pos(&inst),
                Some(_) => {
                    let size = parse_truncate_size(&user[0].value, FN_NAME)?;
                    if size < 0 {
                        return Err(PyError::named(
                            "ValueError",
                            format!("{FN_NAME}(): negative size"),
                        ));
                    }
                    size
                }
            };
            let new_len = string_buffer::truncate(&inst, size, FN_NAME)?;
            Ok(Value::int(new_len))
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
                .insert("_closed", Value::bool_(true));
            Ok(Value::none())
        }

        /// `__enter__` — context manager support; returns `self`.  CPython's
        /// closed-file error here carries the trailing period.
        fn __enter__(args) -> Result<Value> {
            let inst = open_self_dot(args, FN_NAME)?;
            let _ = _interp;
            Ok(Value::py_instance(inst))
        }

        /// `__exit__` — call `close()`.
        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            inst.borrow_mut()
                .attrs
                .insert("_closed", Value::bool_(true));
            Ok(Value::bool_(false))
        }

        /// `writelines(lines)` — write each item of `lines` in order.  No
        /// separators are added (CPython delegates each element to `write`).
        /// Returns `None`.
        fn writelines(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            // CPython's writelines delegates each element to `write` as it
            // iterates, so a non-str element mid-iterable leaves the already
            // written prefix in the buffer (it is NOT atomic).  Write each
            // element as it is validated to match that partial-write behaviour.
            let items = _interp.collect_iterable(&args[1].value)?;
            for item in &items {
                match item.kind() {
                    ValueKind::Str(s) => {
                        string_buffer::write(&inst, s)?;
                    }
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "string argument expected, got '{}'",
                            pyrust_core::builtin_type_name(item)
                        ),
                    )),
                }
            }
            Ok(Value::none())
        }

        /// `seekable()` — StringIO is always seekable.  Raises on a closed
        /// stream (CPython message omits the trailing period here).
        fn seekable(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error()); }
            Ok(Value::bool_(true))
        }

        /// `readable()` — StringIO is always readable.
        fn readable(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error()); }
            Ok(Value::bool_(true))
        }

        /// `writable()` — StringIO is always writable.
        fn writable(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error()); }
            Ok(Value::bool_(true))
        }

        /// `flush()` — no-op for in-memory streams; returns `None`.  CPython's
        /// StringIO.flush() does NOT raise on a closed stream.
        fn flush(args) -> Result<Value> {
            let _ = expect_self(args, FN_NAME)?;
            let _ = _interp;
            Ok(Value::none())
        }

        /// `isatty()` — always `False`.  Raises (with trailing period) when
        /// the stream is closed.
        fn isatty(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            Ok(Value::bool_(false))
        }

        /// `fileno()` — in-memory streams have no file descriptor; CPython
        /// raises `io.UnsupportedOperation` regardless of open/closed state.
        fn fileno(args) -> Result<Value> {
            let _ = expect_self(args, FN_NAME)?;
            let _ = _interp;
            Err(unsupported_fileno())
        }

        /// `closed` — getter backing the `closed` property (wired up in
        /// `env.rs` after the module is built).  Returns the bool state and
        /// never raises.
        fn closed(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            Ok(Value::bool_(is_closed(&inst)))
        }

        /// `__iter__` — line iteration support; returns `self`.
        fn __iter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            Ok(Value::py_instance(inst))
        }

        /// `__next__` — yield the next line (terminated by `\n` or EOF),
        /// raising `StopIteration` once the buffer is exhausted.  CPython's
        /// StringIO.__next__ closed-file error omits the trailing period
        /// (unlike __iter__/isatty, which include it).
        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error()); }
            match string_buffer::next_line(&inst, FN_NAME)? {
                Some(line) => Ok(Value::string(line)),
                None => Err(PyError::named("StopIteration", String::new())),
            }
        }

        fn __repr__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let closed = is_closed(&inst);
            let _ = _interp;
            if closed {
                Ok(Value::string("<_io.StringIO [closed]>"))
            } else {
                Ok(Value::string("<_io.StringIO object>"))
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
            bytes_buffer::initialize(&inst, initial);
            Ok(Value::none())
        }

        /// `read([size=-1])` — read up to `size` bytes.
        fn read(args) -> Result<Value> {
            let inst = open_self_dot(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let size = parse_optional_size(user, FN_NAME)?;
            let _ = _interp;
            Ok(Value::bytes(bytes_buffer::read(&inst, size, FN_NAME)?))
        }

        /// `readline([size=-1])` — read up to the next `\n` byte (inclusive).
        fn readline(args) -> Result<Value> {
            let inst = open_self_dot(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let size_limit = parse_optional_size(user, FN_NAME)?;
            let _ = _interp;
            Ok(Value::bytes(bytes_buffer::read_line(
                &inst, size_limit, FN_NAME,
            )?))
        }

        /// `readlines([hint])` — read all remaining lines into a list of bytes.
        fn readlines(args) -> Result<Value> {
            let inst = open_self_dot(args, FN_NAME)?;
            user_at_most_one(args, FN_NAME)?;
            let _ = _interp;
            let lines = bytes_buffer::read_lines(&inst, FN_NAME)?
                .into_iter()
                .map(Value::bytes)
                .collect();
            Ok(Value::list(lines))
        }

        /// `write(b)` — write bytes at the current position.
        fn write(args) -> Result<Value> {
            let inst = open_self_dot(args, FN_NAME)?;
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
            let n_written = bytes_buffer::write(&inst, &data)?;
            Ok(Value::int(n_written as i64))
        }

        /// `getvalue()` — return the full buffer regardless of position.
        fn getvalue(args) -> Result<Value> {
            let inst = open_self_dot(args, FN_NAME)?;
            if args.len() != 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes no arguments"),
                ));
            }
            let _ = _interp;
            Ok(Value::bytes(bytes_buffer::contents(&inst, FN_NAME)?))
        }

        /// `seek(pos[, whence=0])` — BytesIO supports all three whence modes
        /// with arbitrary offsets (unlike StringIO which restricts them).
        fn seek(args) -> Result<Value> {
            let inst = open_self_dot(args, FN_NAME)?;
            let user = &args[1..];
            if user.is_empty() || user.len() > 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes 1 or 2 arguments"),
                ));
            }
            let offset = parse_io_offset(&user[0].value, FN_NAME, "offset")?;
            let whence = match user.get(1) {
                None => 0i64,
                Some(arg) => parse_io_whence(&arg.value, FN_NAME)?,
            };
            let _ = _interp;
            Ok(Value::int(bytes_buffer::seek(
                &inst, offset, whence, FN_NAME,
            )?))
        }

        /// `tell()` — return the current position (byte offset).
        fn tell(args) -> Result<Value> {
            let inst = open_self_dot(args, FN_NAME)?;
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
            let inst = open_self_dot(args, FN_NAME)?;
            let user = user_at_most_one(args, FN_NAME)?;
            let _ = _interp;
            let size = match user.first().map(|a| a.value.kind()) {
                None | Some(ValueKind::None) => get_pos(&inst),
                Some(_) => {
                    let size = parse_truncate_size(&user[0].value, FN_NAME)?;
                    if size < 0 {
                        return Err(PyError::named(
                            "ValueError",
                            format!("{FN_NAME}(): negative size"),
                        ));
                    }
                    size
                }
            };
            let new_len = bytes_buffer::truncate(&inst, size, FN_NAME)?;
            Ok(Value::int(new_len))
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
                .insert("_closed", Value::bool_(true));
            Ok(Value::none())
        }

        fn __enter__(args) -> Result<Value> {
            let inst = open_self_dot(args, FN_NAME)?;
            let _ = _interp;
            Ok(Value::py_instance(inst))
        }

        fn __exit__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            inst.borrow_mut()
                .attrs
                .insert("_closed", Value::bool_(true));
            Ok(Value::bool_(false))
        }

        /// `writelines(lines)` — write each bytes item in order, no separators.
        fn writelines(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            // CPython delegates each element to `write` as it iterates, so a
            // non-bytes element mid-iterable leaves the written prefix in the
            // buffer (it is NOT atomic).  Write each as it is validated.
            let items = _interp.collect_iterable(&args[1].value)?;
            for item in &items {
                match item.kind() {
                    ValueKind::Bytes(b) => {
                        bytes_buffer::write(&inst, b)?;
                    }
                    _ => match pyrust_builtins::bytearray::as_bytearray_snapshot(item) {
                        Some(b) => {
                            bytes_buffer::write(&inst, &b)?;
                        }
                        None => return Err(PyError::named(
                            "TypeError",
                            format!(
                                "a bytes-like object is required, not '{}'",
                                pyrust_core::builtin_type_name(item)
                            ),
                        )),
                    },
                }
            }
            Ok(Value::none())
        }

        /// `readinto(b)` — read up to `len(b)` bytes into the writable
        /// bytes-like object `b`, returning the number of bytes read.
        fn readinto(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            if args.len() != 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() takes exactly 1 argument"),
                ));
            }
            let target = match pyrust_builtins::bytearray::as_bytearray_rc(&args[1].value) {
                Some(rc) => rc,
                None => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "readinto() argument must be read-write bytes-like object, not {}",
                        pyrust_core::builtin_type_name(&args[1].value)
                    ),
                )),
            };
            let _ = _interp;
            let mut dst = target.borrow_mut();
            let n = bytes_buffer::read_into(&inst, &mut dst, FN_NAME)?;
            Ok(Value::int(n as i64))
        }

        /// `read1([size=-1])` — for an in-memory stream this is identical to
        /// `read`; there is no underlying raw layer to do a single syscall on.
        fn read1(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            let user = user_at_most_one(args, FN_NAME)?;
            let size = parse_optional_size(user, FN_NAME)?;
            let _ = _interp;
            Ok(Value::bytes(bytes_buffer::read(&inst, size, FN_NAME)?))
        }

        /// `seekable()` — BytesIO is always seekable.
        fn seekable(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            Ok(Value::bool_(true))
        }

        /// `readable()` — BytesIO is always readable.
        fn readable(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            Ok(Value::bool_(true))
        }

        /// `writable()` — BytesIO is always writable.
        fn writable(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            Ok(Value::bool_(true))
        }

        /// `flush()` — no-op; returns `None`.  Unlike StringIO, CPython's
        /// BytesIO.flush() DOES raise on a closed stream.
        fn flush(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            Ok(Value::none())
        }

        /// `isatty()` — always `False`.
        fn isatty(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            Ok(Value::bool_(false))
        }

        /// `fileno()` — no file descriptor; raises `io.UnsupportedOperation`.
        fn fileno(args) -> Result<Value> {
            let _ = expect_self(args, FN_NAME)?;
            let _ = _interp;
            Err(unsupported_fileno())
        }

        /// `closed` — getter backing the `closed` property.
        fn closed(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            Ok(Value::bool_(is_closed(&inst)))
        }

        /// `__iter__` — line iteration support; returns `self`.
        fn __iter__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            Ok(Value::py_instance(inst))
        }

        /// `__next__` — yield the next line (terminated by `\n` or EOF),
        /// raising `StopIteration` once the buffer is exhausted.
        fn __next__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let _ = _interp;
            if is_closed(&inst) { return Err(closed_error_dot()); }
            match bytes_buffer::next_line(&inst, FN_NAME)? {
                Some(line) => Ok(Value::bytes(line)),
                None => Err(PyError::named("StopIteration", String::new())),
            }
        }

        fn __repr__(args) -> Result<Value> {
            let inst = expect_self(args, FN_NAME)?;
            let closed = is_closed(&inst);
            let _ = _interp;
            if closed {
                Ok(Value::string("<_io.BytesIO [closed]>"))
            } else {
                Ok(Value::string("<_io.BytesIO object>"))
            }
        }
    }
}
