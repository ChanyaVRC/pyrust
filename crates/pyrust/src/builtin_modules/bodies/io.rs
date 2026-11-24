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
// This file is the thin Python-registration facade. Concrete stream methods,
// shared stream-state helpers, buffer policies, and ABC injection live in
// child modules under `bodies/io/`.
//
// Reference: <https://docs.python.org/3/library/io.html>

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "io/base_classes.rs"]
mod base_classes;
#[path = "io/bytes_buffer.rs"]
mod bytes_buffer;
#[path = "io/bytes_io.rs"]
mod bytes_io;
#[path = "io/open_helpers.rs"]
mod open_helpers;
#[path = "io/string_buffer.rs"]
mod string_buffer;
#[path = "io/string_io.rs"]
mod string_io;

pub(crate) use self::base_classes::inject_python_members;
use self::open_helpers::{
    get_pos, set_pos, stream_bool_query, stream_close, stream_closed, stream_enter, stream_exit,
    stream_fileno, stream_iter, stream_repr, stream_tell,
};

pyrust_module! {
    class StringIO {
        /// `StringIO(initial_value='', newline='\n')` — creates an in-memory text stream.
        /// <https://docs.python.org/3/library/io.html#io.StringIO>
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            string_io::init(args, FN_NAME)
        }

        fn read(args) -> Result<Value> {
            let _ = _interp;
            string_io::read(args, FN_NAME)
        }

        fn readline(args) -> Result<Value> {
            let _ = _interp;
            string_io::readline(args, FN_NAME)
        }

        fn readlines(args) -> Result<Value> {
            let _ = _interp;
            string_io::readlines(args, FN_NAME)
        }

        fn write(args) -> Result<Value> {
            let _ = _interp;
            string_io::write(args, FN_NAME)
        }

        fn getvalue(args) -> Result<Value> {
            let _ = _interp;
            string_io::getvalue(args, FN_NAME)
        }

        fn seek(args) -> Result<Value> {
            let _ = _interp;
            string_io::seek(args, FN_NAME)
        }

        fn tell(args) -> Result<Value> {
            let _ = _interp;
            stream_tell(args, FN_NAME, false)
        }

        fn truncate(args) -> Result<Value> {
            let _ = _interp;
            string_io::truncate(args, FN_NAME)
        }

        fn close(args) -> Result<Value> {
            let _ = _interp;
            stream_close(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            let _ = _interp;
            stream_enter(args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            let _ = _interp;
            stream_exit(args, FN_NAME)
        }

        fn writelines(args) -> Result<Value> {
            string_io::writelines(_interp, args, FN_NAME)
        }

        fn seekable(args) -> Result<Value> {
            let _ = _interp;
            stream_bool_query(args, FN_NAME, false, true)
        }

        fn readable(args) -> Result<Value> {
            let _ = _interp;
            stream_bool_query(args, FN_NAME, false, true)
        }

        fn writable(args) -> Result<Value> {
            let _ = _interp;
            stream_bool_query(args, FN_NAME, false, true)
        }

        fn flush(args) -> Result<Value> {
            let _ = _interp;
            string_io::flush(args, FN_NAME)
        }

        fn isatty(args) -> Result<Value> {
            let _ = _interp;
            stream_bool_query(args, FN_NAME, true, false)
        }

        fn fileno(args) -> Result<Value> {
            let _ = _interp;
            stream_fileno(args, FN_NAME)
        }

        fn closed(args) -> Result<Value> {
            let _ = _interp;
            stream_closed(args, FN_NAME)
        }

        fn __iter__(args) -> Result<Value> {
            let _ = _interp;
            stream_iter(args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            let _ = _interp;
            string_io::next(args, FN_NAME)
        }

        fn __repr__(args) -> Result<Value> {
            let _ = _interp;
            stream_repr(args, FN_NAME, "_io.StringIO")
        }
    }

    class BytesIO {
        /// `BytesIO(initial_bytes=b'')` — creates an in-memory bytes stream.
        /// <https://docs.python.org/3/library/io.html#io.BytesIO>
        fn __init__(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::init(args, FN_NAME)
        }

        fn read(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::read(args, FN_NAME)
        }

        fn readline(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::readline(args, FN_NAME)
        }

        fn readlines(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::readlines(args, FN_NAME)
        }

        fn write(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::write(args, FN_NAME)
        }

        fn getvalue(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::getvalue(args, FN_NAME)
        }

        fn seek(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::seek(args, FN_NAME)
        }

        fn tell(args) -> Result<Value> {
            let _ = _interp;
            stream_tell(args, FN_NAME, true)
        }

        fn truncate(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::truncate(args, FN_NAME)
        }

        fn close(args) -> Result<Value> {
            let _ = _interp;
            stream_close(args, FN_NAME)
        }

        fn __enter__(args) -> Result<Value> {
            let _ = _interp;
            stream_enter(args, FN_NAME)
        }

        fn __exit__(args) -> Result<Value> {
            let _ = _interp;
            stream_exit(args, FN_NAME)
        }

        fn writelines(args) -> Result<Value> {
            bytes_io::writelines(_interp, args, FN_NAME)
        }

        fn readinto(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::readinto(args, FN_NAME)
        }

        fn read1(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::read(args, FN_NAME)
        }

        fn seekable(args) -> Result<Value> {
            let _ = _interp;
            stream_bool_query(args, FN_NAME, true, true)
        }

        fn readable(args) -> Result<Value> {
            let _ = _interp;
            stream_bool_query(args, FN_NAME, true, true)
        }

        fn writable(args) -> Result<Value> {
            let _ = _interp;
            stream_bool_query(args, FN_NAME, true, true)
        }

        fn flush(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::flush(args, FN_NAME)
        }

        fn isatty(args) -> Result<Value> {
            let _ = _interp;
            stream_bool_query(args, FN_NAME, true, false)
        }

        fn fileno(args) -> Result<Value> {
            let _ = _interp;
            stream_fileno(args, FN_NAME)
        }

        fn closed(args) -> Result<Value> {
            let _ = _interp;
            stream_closed(args, FN_NAME)
        }

        fn __iter__(args) -> Result<Value> {
            let _ = _interp;
            stream_iter(args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            let _ = _interp;
            bytes_io::next(args, FN_NAME)
        }

        fn __repr__(args) -> Result<Value> {
            let _ = _interp;
            stream_repr(args, FN_NAME, "_io.BytesIO")
        }
    }
}
