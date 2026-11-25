"""Interface to the pure-Rust bzip2 compression engine.

Python 3.12 ``bz2`` module implementation (issue #3224).
"""

import builtins as _builtins
import io as _io
import os as _os

__all__ = [
    "BZ2File",
    "BZ2Compressor",
    "BZ2Decompressor",
    "open",
    "compress",
    "decompress",
]

_MODE_CLOSED = 0
_MODE_READ = 1
_MODE_WRITE = 3

_READ_CHUNK_SIZE = 8192


def _fspath_str(path):
    p = _os.fspath(path)
    if isinstance(p, bytes):
        return p.decode("utf-8")
    return p


def _open_disk_file(path, mode):
    path_s = _fspath_str(path)
    if "x" in mode:
        if _os.path.exists(path_s):
            raise FileExistsError("File exists: %r" % path_s)
        return _builtins.open(path_s, "wb")
    return _builtins.open(path_s, mode)


class BZ2Compressor:
    """Create a compressor object for compressing data incrementally."""

    def __init__(self, compresslevel=9, /):
        self._state = _compressor_new(compresslevel)

    def compress(self, data, /):
        """Provide data to the compressor object."""
        return _compressor_compress(self._state, data)

    def flush(self):
        """Finish the compression process."""
        return _compressor_flush(self._state)


class BZ2Decompressor:
    """Create a decompressor object for decompressing data incrementally."""

    def __init__(self):
        self._state = _decompressor_new()
        self._eof = False
        self._needs_input = True
        self._unused_data = b""

    @property
    def eof(self):
        return self._eof

    @property
    def needs_input(self):
        return self._needs_input

    @property
    def unused_data(self):
        return self._unused_data

    def decompress(self, data, max_length=-1):
        """Decompress *data*, returning uncompressed data as bytes."""
        out, eof, needs_input, unused_data = _decompressor_decompress(
            self._state, data, max_length
        )
        self._eof = eof
        self._needs_input = needs_input
        self._unused_data = unused_data
        return out


class _BZ2Reader:
    """Streaming multi-stream bzip2 reader."""

    def __init__(self, fp):
        self._fp = fp
        self._decomp = None
        self._raw_buf = b""
        self._out_buf = bytearray()
        self._eof = False
        self._pos = 0
        self._seen_stream = False
        try:
            self._raw_start = fp.tell()
        except Exception:
            self._raw_start = 0

    def _pump(self, size):
        while not self._eof and (size < 0 or len(self._out_buf) < size):
            if self._decomp is None or self._decomp.eof:
                if self._decomp is not None and self._decomp.unused_data:
                    self._raw_buf = self._decomp.unused_data + self._raw_buf
                if not self._raw_buf:
                    chunk = self._fp.read(_READ_CHUNK_SIZE)
                    if not chunk:
                        self._eof = True
                        break
                    self._raw_buf = chunk
                self._decomp = BZ2Decompressor()

            if self._decomp.needs_input:
                if self._raw_buf:
                    raw = self._raw_buf
                    self._raw_buf = b""
                else:
                    raw = self._fp.read(_READ_CHUNK_SIZE)
                    if not raw:
                        raise EOFError(
                            "Compressed file ended before the end-of-stream marker was reached"
                        )
            else:
                raw = b""

            max_len = -1 if size < 0 else max(size - len(self._out_buf), 1)
            try:
                data = self._decomp.decompress(raw, max_length=max_len)
            except OSError:
                if self._seen_stream:
                    # Trailing garbage after a complete stream is ignored (CPython behavior).
                    self._eof = True
                    break
                raise

            if data:
                self._out_buf.extend(data)
            if self._decomp.eof:
                self._seen_stream = True
                if self._decomp.unused_data:
                    self._raw_buf = self._decomp.unused_data + self._raw_buf
                self._decomp = None

    def read(self, size=-1):
        if size == 0:
            return b""
        if size is None or size < 0:
            self._pump(-1)
            data = bytes(self._out_buf)
            self._out_buf.clear()
            self._pos += len(data)
            return data
        self._pump(size)
        data = bytes(self._out_buf[:size])
        del self._out_buf[: len(data)]
        self._pos += len(data)
        return data

    def peek(self, n=0):
        if not self._out_buf and not self._eof:
            self._pump(max(n, 1))
        return bytes(self._out_buf)

    def readline(self, size=-1):
        if size is None:
            size = -1
        if size == 0:
            return b""
        out = bytearray()
        while True:
            if not self._out_buf:
                if self._eof:
                    break
                self._pump(1)
                if not self._out_buf:
                    break
            nl = self._out_buf.find(b"\n")
            if nl >= 0:
                take = nl + 1
                if size > 0:
                    take = min(take, size - len(out))
                out.extend(self._out_buf[:take])
                del self._out_buf[:take]
                self._pos += take
                break
            if size > 0 and len(out) + len(self._out_buf) >= size:
                take = size - len(out)
                out.extend(self._out_buf[:take])
                del self._out_buf[:take]
                self._pos += take
                break
            take = len(self._out_buf)
            out.extend(self._out_buf)
            self._out_buf.clear()
            self._pos += take
        return bytes(out)

    def _rewind(self):
        self._fp.seek(self._raw_start)
        self._decomp = None
        self._raw_buf = b""
        self._out_buf.clear()
        self._eof = False
        self._pos = 0
        self._seen_stream = False

    def seek(self, offset, whence=_io.SEEK_SET):
        if whence == _io.SEEK_SET:
            target = offset
        elif whence == _io.SEEK_CUR:
            target = self._pos + offset
        elif whence == _io.SEEK_END:
            while self.read(_READ_CHUNK_SIZE):
                pass
            target = self._pos + offset
        else:
            raise ValueError("Invalid value for whence: %r" % (whence,))
        if target < 0:
            target = 0
        if target < self._pos:
            self._rewind()
        skip = target - self._pos
        while skip > 0:
            chunk = self.read(min(skip, _READ_CHUNK_SIZE))
            if not chunk:
                break
            skip -= len(chunk)
        return self._pos


class BZ2File(_io.BufferedIOBase):
    """A file object providing transparent bzip2 (de)compression."""

    def __init__(self, filename, mode="r", *, compresslevel=9):
        self._fp = None
        self._closefp = False
        self._mode = _MODE_CLOSED
        self._pos = 0
        self._name = None

        if not (1 <= compresslevel <= 9):
            raise ValueError("compresslevel must be between 1 and 9")

        if mode in ("", "r", "rb"):
            mode = "rb"
            mode_code = _MODE_READ
        elif mode in ("w", "wb"):
            mode = "wb"
            mode_code = _MODE_WRITE
            self._compressor = BZ2Compressor(compresslevel)
        elif mode in ("x", "xb"):
            mode = "xb"
            mode_code = _MODE_WRITE
            self._compressor = BZ2Compressor(compresslevel)
        elif mode in ("a", "ab"):
            mode = "ab"
            mode_code = _MODE_WRITE
            self._compressor = BZ2Compressor(compresslevel)
        else:
            raise ValueError("Invalid mode: %r" % (mode,))

        if isinstance(filename, (str, bytes)) or hasattr(filename, "__fspath__"):
            self._fp = _open_disk_file(filename, mode)
            self._closefp = True
            self._name = _os.fspath(filename)
        elif hasattr(filename, "read") or hasattr(filename, "write"):
            self._fp = filename
            self._name = getattr(filename, "name", None)
        else:
            raise TypeError("filename must be a str, bytes, file or PathLike object")

        self._mode = mode_code
        if self._mode == _MODE_READ:
            self._reader = _BZ2Reader(self._fp)

    @property
    def closed(self):
        return self._mode == _MODE_CLOSED

    @property
    def name(self):
        return self._name

    def _check_not_closed(self):
        if self.closed:
            raise ValueError("I/O operation on closed file")

    def fileno(self):
        self._check_not_closed()
        return self._fp.fileno()

    def seekable(self):
        return self.readable() and (
            self._fp.seekable() if hasattr(self._fp, "seekable") else True
        )

    def readable(self):
        self._check_not_closed()
        return self._mode == _MODE_READ

    def writable(self):
        self._check_not_closed()
        return self._mode == _MODE_WRITE

    def peek(self, n=0):
        self._check_not_closed()
        if self._mode != _MODE_READ:
            raise _io.UnsupportedOperation("File not open for reading")
        return self._reader.peek(n)

    def read(self, size=-1):
        self._check_not_closed()
        if self._mode != _MODE_READ:
            raise _io.UnsupportedOperation("File not open for reading")
        data = self._reader.read(size)
        self._pos = self._reader._pos
        return data

    def read1(self, size=-1):
        self._check_not_closed()
        if self._mode != _MODE_READ:
            raise _io.UnsupportedOperation("File not open for reading")
        if size is None or size < 0:
            size = _io.DEFAULT_BUFFER_SIZE
        data = self._reader.read(size)
        self._pos = self._reader._pos
        return data

    def readinto(self, b):
        self._check_not_closed()
        if self._mode != _MODE_READ:
            raise _io.UnsupportedOperation("File not open for reading")
        data = self.read(len(b))
        n = len(data)
        b[:n] = data
        return n

    def readline(self, size=-1):
        self._check_not_closed()
        if self._mode != _MODE_READ:
            raise _io.UnsupportedOperation("File not open for reading")
        line = self._reader.readline(size)
        self._pos = self._reader._pos
        return line

    def readlines(self, size=-1):
        self._check_not_closed()
        if self._mode != _MODE_READ:
            raise _io.UnsupportedOperation("File not open for reading")
        if size is None:
            size = -1
        lines = []
        total = 0
        while True:
            line = self.readline()
            if not line:
                break
            lines.append(line)
            total += len(line)
            if size > 0 and total >= size:
                break
        return lines

    def write(self, data):
        self._check_not_closed()
        if self._mode != _MODE_WRITE:
            raise _io.UnsupportedOperation("File not open for writing")
        compressed = self._compressor.compress(data)
        if compressed:
            self._fp.write(compressed)
        if isinstance(data, (bytes, bytearray)):
            length = len(data)
        else:
            length = len(bytes(data))
        self._pos += length
        return length

    def writelines(self, seq):
        self._check_not_closed()
        if self._mode != _MODE_WRITE:
            raise _io.UnsupportedOperation("File not open for writing")
        for line in seq:
            self.write(line)

    def seek(self, offset, whence=_io.SEEK_SET):
        self._check_not_closed()
        if self._mode != _MODE_READ:
            raise _io.UnsupportedOperation("Seek is only supported in read mode")
        pos = self._reader.seek(offset, whence)
        self._pos = pos
        return pos

    def tell(self):
        self._check_not_closed()
        if self._mode == _MODE_READ:
            return self._reader._pos
        return self._pos

    def flush(self):
        self._check_not_closed()
        if self._mode == _MODE_WRITE and hasattr(self._fp, "flush"):
            self._fp.flush()

    def close(self):
        if self._mode == _MODE_CLOSED:
            return
        try:
            if self._mode == _MODE_WRITE:
                flushed = self._compressor.flush()
                if flushed:
                    self._fp.write(flushed)
                self._compressor = None
        finally:
            try:
                if self._closefp and self._fp is not None:
                    self._fp.close()
            finally:
                self._fp = None
                self._closefp = False
                self._mode = _MODE_CLOSED

    def __iter__(self):
        return self

    def __next__(self):
        line = self.readline()
        if not line:
            raise StopIteration
        return line

    def __enter__(self):
        self._check_not_closed()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False


class _TextBZ2Wrapper(_io.TextIOBase):
    """Text-mode wrapper around a binary ``BZ2File`` for ``bz2.open(..., 'rt'/'wt'/'at'/'xt')``."""

    def __init__(self, buffer, encoding=None, errors=None, newline=None):
        if newline not in (None, "", "\n", "\r", "\r\n"):
            raise ValueError("illegal newline value: %r" % (newline,))
        self.buffer = buffer
        self._encoding = encoding or "utf-8"
        self._errors = errors or "strict"
        self._newline = newline
        self._read_buf = ""
        self._eof = False

    @property
    def encoding(self):
        return self._encoding

    @property
    def errors(self):
        return self._errors

    @property
    def name(self):
        return self.buffer.name

    @property
    def closed(self):
        return self.buffer.closed

    def readable(self):
        return self.buffer.readable()

    def writable(self):
        return self.buffer.writable()

    def seekable(self):
        return self.buffer.seekable()

    def _translate_read(self, text):
        if self._newline is None:
            text = text.replace("\r\n", "\n").replace("\r", "\n")
        return text

    def _translate_write(self, text):
        if self._newline is None:
            if _os.linesep != "\n":
                text = text.replace("\n", _os.linesep)
        elif self._newline not in ("", "\n"):
            text = text.replace("\n", self._newline)
        return text

    def _fill_text(self):
        if self._eof:
            return
        raw = self.buffer.read()
        self._eof = True
        if raw:
            text = raw.decode(self._encoding, self._errors)
            self._read_buf += self._translate_read(text)

    def read(self, size=-1):
        if self.closed:
            raise ValueError("I/O operation on closed file")
        if not self.readable():
            raise _io.UnsupportedOperation("not readable")
        if size is None or size < 0:
            self._fill_text()
            res = self._read_buf
            self._read_buf = ""
            return res
        if size == 0:
            return ""
        if len(self._read_buf) < size and not self._eof:
            self._fill_text()
        res = self._read_buf[:size]
        self._read_buf = self._read_buf[size:]
        return res

    def readline(self, size=-1):
        if self.closed:
            raise ValueError("I/O operation on closed file")
        if not self.readable():
            raise _io.UnsupportedOperation("not readable")
        if size is None:
            size = -1
        if size == 0:
            return ""
        if not self._eof:
            self._fill_text()
        if not self._read_buf:
            return ""
        if self._newline in (None, "\n"):
            idx = self._read_buf.find("\n")
            end = idx + 1 if idx >= 0 else len(self._read_buf)
        elif self._newline == "":
            n = len(self._read_buf)
            end = n
            i = 0
            while i < n:
                ch = self._read_buf[i]
                if ch == "\n":
                    end = i + 1
                    break
                if ch == "\r":
                    if i + 1 < n and self._read_buf[i + 1] == "\n":
                        end = i + 2
                    else:
                        end = i + 1
                    break
                i += 1
        else:
            sep = self._newline
            idx = self._read_buf.find(sep)
            end = idx + len(sep) if idx >= 0 else len(self._read_buf)
        if size > 0 and end > size:
            end = size
        res = self._read_buf[:end]
        self._read_buf = self._read_buf[end:]
        return res

    def readlines(self, hint=-1):
        if hint is None:
            hint = -1
        lines = []
        total = 0
        while True:
            line = self.readline()
            if not line:
                break
            lines.append(line)
            total += len(line)
            if hint > 0 and total >= hint:
                break
        return lines

    def write(self, s):
        if self.closed:
            raise ValueError("I/O operation on closed file")
        if not self.writable():
            raise _io.UnsupportedOperation("not writable")
        if not isinstance(s, str):
            raise TypeError("write() argument must be str, not %s" % type(s).__name__)
        translated = self._translate_write(s)
        self.buffer.write(translated.encode(self._encoding, self._errors))
        return len(s)

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def flush(self):
        self.buffer.flush()

    def tell(self):
        return self.buffer.tell()

    def seek(self, offset, whence=_io.SEEK_SET):
        self._read_buf = ""
        self._eof = False
        return self.buffer.seek(offset, whence)

    def close(self):
        self.buffer.close()

    def __iter__(self):
        return self

    def __next__(self):
        line = self.readline()
        if not line:
            raise StopIteration
        return line

    def __enter__(self):
        if self.closed:
            raise ValueError("I/O operation on closed file")
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False


def open(
    filename,
    mode="rb",
    compresslevel=9,
    encoding=None,
    errors=None,
    newline=None,
):
    """Open a bzip2-compressed file in binary or text mode."""
    if "t" in mode:
        if "b" in mode:
            raise ValueError("Invalid mode: %r" % (mode,))
    else:
        if encoding is not None:
            raise ValueError("Argument 'encoding' not supported in binary mode")
        if errors is not None:
            raise ValueError("Argument 'errors' not supported in binary mode")
        if newline is not None:
            raise ValueError("Argument 'newline' not supported in binary mode")

    bz_mode = mode.replace("t", "")
    binary_file = BZ2File(filename, bz_mode, compresslevel=compresslevel)

    if "t" in mode:
        return _TextBZ2Wrapper(
            binary_file, encoding=encoding, errors=errors, newline=newline
        )
    return binary_file


def compress(data, compresslevel=9):
    """Compress a block of data in one shot."""
    comp = BZ2Compressor(compresslevel)
    return comp.compress(data) + comp.flush()


def decompress(data):
    """Decompress a block of data in one shot (supports multi-stream bz2)."""
    results = []
    while data:
        decomp = BZ2Decompressor()
        try:
            res = decomp.decompress(data)
        except OSError:
            if results:
                break
            raise
        results.append(res)
        if not decomp.eof:
            raise ValueError(
                "Compressed data ended before the end-of-stream marker was reached"
            )
        data = decomp.unused_data
    return b"".join(results)
