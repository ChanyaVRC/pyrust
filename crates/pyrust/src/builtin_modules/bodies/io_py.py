# Python-level supplements to the native `io` module (issue #2778), injected by
# `io.rs::inject_python_members` (mirrors `operator` / `enum` / `string`).
#
# The native `pyrust_module!` block in `io.rs` provides the concrete in-memory
# stream classes `StringIO` and `BytesIO` plus `io.UnsupportedOperation`.  This
# file adds the remaining public surface CPython's `io` module exposes:
#
#   - the seek-whence constants `SEEK_SET` / `SEEK_CUR` / `SEEK_END`,
#   - `DEFAULT_BUFFER_SIZE`,
#   - `open` (an alias for the built-in `open`),
#   - the abstract base classes `IOBase`, `RawIOBase`, `BufferedIOBase`, and
#     `TextIOBase`.
#
# `io.rs::inject_python_members` re-parents the native `BytesIO` onto
# `BufferedIOBase` and `StringIO` onto `TextIOBase` after this source runs, so
# `isinstance(io.BytesIO(), io.BufferedIOBase)` and
# `isinstance(io.StringIO(), io.TextIOBase)` (and, transitively, `io.IOBase`)
# match CPython.
#
# Reference: <https://docs.python.org/3/library/io.html>

# Seek-whence constants.
SEEK_SET = 0
SEEK_CUR = 1
SEEK_END = 2

# Default buffer size used by the buffered I/O classes (CPython: 8192).
DEFAULT_BUFFER_SIZE = 8192

# `io.open` is the same object as the built-in `open` in CPython.
open = open


class IOBase:
    """Abstract base class for all I/O classes."""


class RawIOBase(IOBase):
    """Base class for raw binary streams."""


class BufferedIOBase(IOBase):
    """Base class for binary streams that support some kind of buffering."""


class TextIOBase(IOBase):
    """Base class for text streams."""
