"""Interface to the pure-Rust LZMA/XZ compression engine.

Python 3.12 ``lzma`` module implementation (issue #3224).
"""

import builtins as _builtins
import io as _io
import os as _os
import struct as _struct

__all__ = [
    "CHECK_NONE",
    "CHECK_CRC32",
    "CHECK_CRC64",
    "CHECK_SHA256",
    "CHECK_ID_MAX",
    "CHECK_UNKNOWN",
    "FILTER_LZMA1",
    "FILTER_LZMA2",
    "FILTER_DELTA",
    "FILTER_X86",
    "FILTER_IA64",
    "FILTER_ARM",
    "FILTER_ARMTHUMB",
    "FILTER_POWERPC",
    "FILTER_SPARC",
    "FORMAT_AUTO",
    "FORMAT_XZ",
    "FORMAT_ALONE",
    "FORMAT_RAW",
    "MF_HC3",
    "MF_HC4",
    "MF_BT2",
    "MF_BT3",
    "MF_BT4",
    "MODE_FAST",
    "MODE_NORMAL",
    "PRESET_DEFAULT",
    "PRESET_EXTREME",
    "LZMACompressor",
    "LZMADecompressor",
    "LZMAFile",
    "LZMAError",
    "open",
    "compress",
    "decompress",
    "is_check_supported",
]

FORMAT_AUTO = 0
FORMAT_XZ = 1
FORMAT_ALONE = 2
FORMAT_RAW = 3

CHECK_NONE = 0
CHECK_CRC32 = 1
CHECK_CRC64 = 4
CHECK_SHA256 = 10
CHECK_ID_MAX = 15
CHECK_UNKNOWN = 16

FILTER_LZMA1 = 0x4000000000000001
FILTER_LZMA2 = 0x21
FILTER_DELTA = 0x03
FILTER_X86 = 0x04
FILTER_POWERPC = 0x05
FILTER_IA64 = 0x06
FILTER_ARM = 0x07
FILTER_ARMTHUMB = 0x08
FILTER_SPARC = 0x09

MF_HC3 = 3
MF_HC4 = 4
MF_BT2 = 18
MF_BT3 = 19
MF_BT4 = 20

MODE_FAST = 1
MODE_NORMAL = 2

PRESET_DEFAULT = 6
PRESET_EXTREME = 0x80000000

_MODE_CLOSED = 0
_MODE_READ = 1
_MODE_WRITE = 3

_READ_CHUNK_SIZE = 8192

_PRESET_DICT_SIZES = (
    1 << 18,
    1 << 20,
    1 << 21,
    1 << 22,
    1 << 22,
    1 << 23,
    1 << 23,
    1 << 24,
    1 << 25,
    1 << 26,
)


class LZMAError(Exception):
    """Call to liblzma failed."""


def is_check_supported(check_id, /):
    """Test whether the given integrity check is supported."""
    return _is_check_supported(check_id)


def _encode_filter_properties(filter_spec):
    """Encode a filter specifier dict into its raw liblzma properties bytes."""
    if not isinstance(filter_spec, dict):
        raise TypeError("Filter specifier must be a dict")
    if "id" not in filter_spec:
        raise ValueError('Filter specifier must have an "id" entry')
    fid = filter_spec["id"]
    if fid == FILTER_LZMA1:
        preset = filter_spec.get("preset", PRESET_DEFAULT)
        if preset is None:
            preset = PRESET_DEFAULT
        level = preset & ~PRESET_EXTREME
        if level < 0 or level > 9:
            raise LZMAError("Invalid compression preset: %r" % (preset,))
        lc = filter_spec.get("lc", 3)
        lp = filter_spec.get("lp", 0)
        pb = filter_spec.get("pb", 2)
        dict_size = filter_spec.get("dict_size", _PRESET_DICT_SIZES[level])
        if lc < 0 or lc > 4 or lp < 0 or lp > 4 or lc + lp > 4 or pb < 0 or pb > 4:
            raise LZMAError("Invalid LZMA1 filter properties")
        props_byte = (pb * 5 + lp) * 9 + lc
        return _struct.pack("<BI", props_byte, dict_size)
    if fid == FILTER_LZMA2:
        preset = filter_spec.get("preset", PRESET_DEFAULT)
        if preset is None:
            preset = PRESET_DEFAULT
        level = preset & ~PRESET_EXTREME
        if level < 0 or level > 9:
            raise LZMAError("Invalid compression preset: %r" % (preset,))
        dict_size = filter_spec.get("dict_size", _PRESET_DICT_SIZES[level])
        if dict_size < 4096:
            dict_size = 4096
        for p in range(41):
            if p == 40:
                cap = 0xFFFFFFFF
            else:
                cap = (2 | (p & 1)) << (p // 2 + 11)
            if cap >= dict_size:
                return bytes([p])
        raise LZMAError("Invalid dictionary size")
    if fid == FILTER_DELTA:
        dist = filter_spec.get("dist", 1)
        if dist < 1 or dist > 256:
            raise LZMAError("Invalid delta distance")
        return bytes([dist - 1])
    if fid in (
        FILTER_X86,
        FILTER_POWERPC,
        FILTER_IA64,
        FILTER_ARM,
        FILTER_ARMTHUMB,
        FILTER_SPARC,
    ):
        start_offset = filter_spec.get("start_offset", 0)
        if start_offset == 0:
            return b""
        return _struct.pack("<I", start_offset)
    raise ValueError("Invalid filter ID: %r" % (fid,))


def _decode_filter_properties(filter_id, encoded_props):
    """Decode raw filter properties bytes into a filter specifier dict."""
    if not isinstance(encoded_props, (bytes, bytearray)):
        encoded_props = bytes(encoded_props)
    if filter_id == FILTER_LZMA1:
        if len(encoded_props) != 5:
            raise LZMAError("Invalid LZMA1 properties length")
        props_byte, dict_size = _struct.unpack("<BI", encoded_props)
        if props_byte > 224:
            raise LZMAError("Invalid LZMA1 properties byte")
        pb = props_byte // 45
        rem = props_byte % 45
        lp = rem // 9
        lc = rem % 9
        return {
            "id": FILTER_LZMA1,
            "lc": lc,
            "lp": lp,
            "pb": pb,
            "dict_size": dict_size,
        }
    if filter_id == FILTER_LZMA2:
        if len(encoded_props) != 1:
            raise LZMAError("Invalid LZMA2 properties length")
        p = encoded_props[0]
        if p > 40:
            raise LZMAError("Invalid LZMA2 properties byte")
        if p == 40:
            dict_size = 0xFFFFFFFF
        else:
            dict_size = (2 | (p & 1)) << (p // 2 + 11)
        return {"id": FILTER_LZMA2, "dict_size": dict_size}
    if filter_id == FILTER_DELTA:
        if len(encoded_props) != 1:
            raise LZMAError("Invalid Delta properties length")
        return {"id": FILTER_DELTA, "dist": encoded_props[0] + 1}
    if filter_id in (
        FILTER_X86,
        FILTER_POWERPC,
        FILTER_IA64,
        FILTER_ARM,
        FILTER_ARMTHUMB,
        FILTER_SPARC,
    ):
        if len(encoded_props) == 0:
            return {"id": filter_id}
        if len(encoded_props) == 4:
            (start_offset,) = _struct.unpack("<I", encoded_props)
            return {"id": filter_id, "start_offset": start_offset}
        raise LZMAError("Invalid BCJ properties length")
    raise ValueError("Invalid filter ID: %r" % (filter_id,))


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


class LZMACompressor:
    """Create a compressor object for compressing data incrementally."""

    def __init__(self, format=FORMAT_XZ, check=-1, preset=None, filters=None):
        self._state = _compressor_new(LZMAError, format, check, preset, filters)

    def compress(self, data, /):
        """Provide data to the compressor object."""
        return _compressor_compress(LZMAError, self._state, data)

    def flush(self):
        """Finish the compression process."""
        return _compressor_flush(LZMAError, self._state)


class LZMADecompressor:
    """Create a decompressor object for decompressing data incrementally."""

    def __init__(self, format=FORMAT_AUTO, memlimit=None, filters=None):
        state, init_check = _decompressor_new(LZMAError, format, memlimit, filters)
        self._state = state
        self._check = init_check
        self._eof = False
        self._needs_input = True
        self._unused_data = b""

    @property
    def check(self):
        return self._check

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
        out, check, eof, needs_input, unused_data = _decompressor_decompress(
            LZMAError, self._state, data, max_length
        )
        self._check = check
        self._eof = eof
        self._needs_input = needs_input
        self._unused_data = unused_data
        return out


class _LZMAReader:
    """Streaming multi-stream XZ/LZMA reader."""

    def __init__(self, fp, format, memlimit, filters):
        self._fp = fp
        self._format = format
        self._memlimit = memlimit
        self._filters = filters
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
                self._decomp = LZMADecompressor(
                    self._format, self._memlimit, self._filters
                )

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
            except LZMAError:
                if self._seen_stream:
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


class LZMAFile(_io.BufferedIOBase):
    """A file object providing transparent LZMA (de)compression."""

    def __init__(
        self,
        filename=None,
        mode="r",
        *,
        format=None,
        check=-1,
        preset=None,
        filters=None,
    ):
        self._fp = None
        self._closefp = False
        self._mode = _MODE_CLOSED
        self._pos = 0
        self._name = None

        if mode in ("r", "rb"):
            if check != -1:
                raise ValueError("Cannot specify an integrity check when opening a file for reading")
            if preset is not None:
                raise ValueError("Cannot specify a preset compression level when opening a file for reading")
            if format is None:
                format = FORMAT_AUTO
            mode_code = _MODE_READ
        elif mode in ("w", "wb", "a", "ab", "x", "xb"):
            if format is None:
                format = FORMAT_XZ
            mode_code = _MODE_WRITE
            self._compressor = LZMACompressor(
                format=format, check=check, preset=preset, filters=filters
            )
        else:
            raise ValueError("Invalid mode: {!r}".format(mode))

        if isinstance(filename, (str, bytes)) or hasattr(filename, "__fspath__"):
            if "b" not in mode:
                mode += "b"
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
            # Validate format/filters eagerly just like CPython.
            LZMADecompressor(format, None, filters)
            self._reader = _LZMAReader(self._fp, format, None, filters)

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

    def peek(self, size=-1):
        self._check_not_closed()
        if self._mode != _MODE_READ:
            raise _io.UnsupportedOperation("File not open for reading")
        return self._reader.peek(max(size, 1) if size is not None else 1)

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

    def readlines(self, hint=-1):
        self._check_not_closed()
        if self._mode != _MODE_READ:
            raise _io.UnsupportedOperation("File not open for reading")
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

    def writelines(self, lines):
        self._check_not_closed()
        if self._mode != _MODE_WRITE:
            raise _io.UnsupportedOperation("File not open for writing")
        for line in lines:
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


class _TextLZMAWrapper(_io.TextIOBase):
    """Text-mode wrapper around a binary ``LZMAFile`` for ``lzma.open(..., 'rt'/'wt'/'at'/'xt')``."""

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
    *,
    format=None,
    check=-1,
    preset=None,
    filters=None,
    encoding=None,
    errors=None,
    newline=None,
):
    """Open an LZMA-compressed file in binary or text mode."""
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

    lz_mode = mode.replace("t", "")
    binary_file = LZMAFile(
        filename,
        lz_mode,
        format=format,
        check=check,
        preset=preset,
        filters=filters,
    )

    if "t" in mode:
        return _TextLZMAWrapper(
            binary_file, encoding=encoding, errors=errors, newline=newline
        )
    return binary_file


def compress(data, format=FORMAT_XZ, check=-1, preset=None, filters=None):
    """Compress a block of data in one shot."""
    comp = LZMACompressor(format, check, preset, filters)
    return comp.compress(data) + comp.flush()


def decompress(data, format=FORMAT_AUTO, memlimit=None, filters=None):
    """Decompress a block of data in one shot (supports multi-stream XZ/LZMA)."""
    results = []
    while True:
        decomp = LZMADecompressor(format, memlimit, filters)
        try:
            res = decomp.decompress(data)
        except LZMAError:
            if results:
                break
            raise
        results.append(res)
        if not decomp.eof:
            raise LZMAError(
                "Compressed data ended before the end-of-stream marker was reached"
            )
        data = decomp.unused_data
        if not data:
            break
    return b"".join(results)
