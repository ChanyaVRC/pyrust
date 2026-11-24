"""Functions that read and write gzipped files.

Python 3.12 ``gzip`` module implementation built on ``zlib``, ``io``, ``os``,
``struct``, and ``time`` (issue #3186).
"""

import builtins as _builtins
import io as _io
import os as _os
import struct as _struct
import time as _time
import zlib as _zlib

__all__ = ["BadGzipFile", "GzipFile", "open", "compress", "decompress"]

FTEXT, FHCRC, FEXTRA, FNAME, FCOMMENT = 1, 2, 4, 8, 16

READ, WRITE = 1, 2

_COMPRESS_LEVEL_FAST = 1
_COMPRESS_LEVEL_TRADEOFF = 6
_COMPRESS_LEVEL_BEST = 9

_READ_CHUNK_SIZE = 8192


class BadGzipFile(OSError):
    """Exception raised in some cases for invalid gzip files."""


def _fspath_str(path):
    p = _os.fspath(path)
    if isinstance(p, bytes):
        return p.decode("utf-8")
    return p


def _to_bytes(data):
    if isinstance(data, bytes):
        return data
    if isinstance(data, bytearray):
        return bytes(data)
    try:
        return bytes(data)
    except TypeError:
        raise TypeError(
            "a bytes-like object is required, not %r" % type(data).__name__
        )


def _open_disk_file(path, mode):
    path_s = _fspath_str(path)
    if "x" in mode:
        if _os.path.exists(path_s):
            raise FileExistsError("File exists: %r" % path_s)
        return _builtins.open(path_s, "wb")
    return _builtins.open(path_s, mode)


def _write_gzip_header(fileobj, fname, compresslevel, mtime):
    fileobj.write(b"\x1f\x8b")
    fileobj.write(b"\x08")
    if isinstance(fname, str):
        fname_bytes = fname.encode("latin-1")
    elif isinstance(fname, (bytes, bytearray)):
        fname_bytes = bytes(fname)
    else:
        fname_bytes = b""
    if fname_bytes.endswith(b".gz"):
        fname_bytes = fname_bytes[:-3]
    flags = FNAME if fname_bytes else 0
    fileobj.write(bytes([flags]))
    if mtime is None:
        mtime = _time.time()
    fileobj.write(_struct.pack("<I", int(mtime) & 0xFFFFFFFF))
    if compresslevel == _COMPRESS_LEVEL_BEST:
        xfl = b"\x02"
    elif compresslevel == _COMPRESS_LEVEL_FAST:
        xfl = b"\x04"
    else:
        xfl = b"\x00"
    fileobj.write(xfl)
    fileobj.write(b"\xff")
    if fname_bytes:
        fileobj.write(fname_bytes + b"\x00")


class _GzipReader:
    """Streaming multi-member RFC 1952 gzip decompressor."""

    def __init__(self, fp):
        self._fp = fp
        self._buf = b""
        self._buf_pos = 0
        self._decomp = None
        self._crc = 0
        self._stream_size = 0
        self._eof = False
        self._new_member = True
        self._last_mtime = None
        self._pos = 0
        self._out_buf = bytearray()
        self._raw_start = 0
        try:
            self._raw_start = fp.tell()
        except Exception:
            self._raw_start = 0

    def _fill_raw(self, n=_READ_CHUNK_SIZE):
        if self._buf_pos > 0:
            self._buf = self._buf[self._buf_pos :]
            self._buf_pos = 0
        chunk = self._fp.read(max(n, _READ_CHUNK_SIZE))
        if chunk:
            self._buf += chunk
        return len(self._buf) - self._buf_pos

    def _read_exact(self, n):
        while (len(self._buf) - self._buf_pos) < n:
            prev = len(self._buf) - self._buf_pos
            self._fill_raw(n - prev)
            if (len(self._buf) - self._buf_pos) == prev:
                raise EOFError(
                    "Compressed file ended before the end-of-stream marker was reached"
                )
        end = self._buf_pos + n
        res = self._buf[self._buf_pos : end]
        self._buf_pos = end
        return res

    def _read_until_nul(self):
        out = bytearray()
        while True:
            b = self._read_exact(1)
            out.append(b[0])
            if b == b"\x00":
                return bytes(out)

    def _peek_avail(self):
        if self._buf_pos >= len(self._buf):
            self._fill_raw()
        return self._buf[self._buf_pos :]

    def _read_gzip_header(self):
        # Skip zero-byte padding between/after members (RFC 1952 / CPython behavior).
        while True:
            avail = self._peek_avail()
            if not avail:
                return False
            if avail[0] == 0:
                self._buf_pos += 1
                continue
            break

        magic = self._read_exact(2)
        if magic != b"\x1f\x8b":
            raise BadGzipFile("Not a gzipped file (%r)" % (magic,))

        fixed = self._read_exact(8)
        method, flag, last_mtime = _struct.unpack("<BBI", fixed[:6])
        if method != 8:
            raise BadGzipFile("Unknown compression method")
        self._last_mtime = last_mtime

        header_crc = _zlib.crc32(magic + fixed)

        if flag & FEXTRA:
            xlen_bytes = self._read_exact(2)
            header_crc = _zlib.crc32(xlen_bytes, header_crc)
            (xlen,) = _struct.unpack("<H", xlen_bytes)
            extra = self._read_exact(xlen)
            header_crc = _zlib.crc32(extra, header_crc)

        if flag & FNAME:
            fname = self._read_until_nul()
            header_crc = _zlib.crc32(fname, header_crc)

        if flag & FCOMMENT:
            fcomment = self._read_until_nul()
            header_crc = _zlib.crc32(fcomment, header_crc)

        if flag & FHCRC:
            hcrc_bytes = self._read_exact(2)
            (hcrc,) = _struct.unpack("<H", hcrc_bytes)
            if hcrc != (header_crc & 0xFFFF):
                raise BadGzipFile("Corrupted gzip header CRC")

        self._decomp = _zlib.decompressobj(-_zlib.MAX_WBITS)
        self._crc = 0
        self._stream_size = 0
        return True

    def _read_trailer(self):
        trailer = self._read_exact(8)
        crc32, isize = _struct.unpack("<II", trailer)
        if crc32 != self._crc:
            raise BadGzipFile(
                "CRC check failed %s != %s" % (hex(crc32), hex(self._crc))
            )
        if isize != (self._stream_size & 0xFFFFFFFF):
            raise BadGzipFile("Incorrect length of data produced")

    def _pump(self, size):
        """Ensure at least `size` bytes (or all remaining if size < 0) in `_out_buf`."""
        while not self._eof and (size < 0 or len(self._out_buf) < size):
            if self._new_member:
                if not self._read_gzip_header():
                    self._eof = True
                    break
                self._new_member = False

            if self._decomp.unconsumed_tail:
                chunk = self._decomp.unconsumed_tail
            else:
                if self._buf_pos >= len(self._buf):
                    self._fill_raw()
                if self._buf_pos >= len(self._buf):
                    raise EOFError(
                        "Compressed file ended before the end-of-stream marker was reached"
                    )
                chunk = self._buf[self._buf_pos :]
                self._buf_pos = len(self._buf)

            max_len = 0 if size < 0 else max(size - len(self._out_buf), 1)
            try:
                uncompressed = self._decomp.decompress(chunk, max_len)
            except _zlib.error as exc:
                raise BadGzipFile(str(exc)) from exc

            if uncompressed:
                self._crc = _zlib.crc32(uncompressed, self._crc)
                self._stream_size += len(uncompressed)
                self._out_buf.extend(uncompressed)

            if self._decomp.eof:
                # Push unused data back into raw buffer so trailer & next member can be read.
                unused = self._decomp.unused_data
                if unused:
                    rem = self._buf[self._buf_pos :]
                    self._buf = unused + rem
                    self._buf_pos = 0
                self._read_trailer()
                self._new_member = True

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

    def peek(self, n=1):
        if n <= 0:
            n = 1
        if not self._out_buf and not self._eof:
            self._pump(n)
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

    def rewind(self):
        self._fp.seek(self._raw_start)
        self._buf = b""
        self._buf_pos = 0
        self._decomp = None
        self._crc = 0
        self._stream_size = 0
        self._eof = False
        self._new_member = True
        self._pos = 0
        self._out_buf.clear()

    def seek(self, offset, whence=_io.SEEK_SET):
        if whence == _io.SEEK_SET:
            target = offset
        elif whence == _io.SEEK_CUR:
            target = self._pos + offset
        elif whence == _io.SEEK_END:
            # Read to end to know total uncompressed length.
            while self.read(_READ_CHUNK_SIZE):
                pass
            target = self._pos + offset
        else:
            raise ValueError("Invalid value for whence: %r" % (whence,))
        if target < 0:
            raise ValueError("Negative seek position %r" % (target,))
        if target < self._pos:
            self.rewind()
        skip = target - self._pos
        while skip > 0:
            chunk = self.read(min(skip, _READ_CHUNK_SIZE))
            if not chunk:
                break
            skip -= len(chunk)
        return self._pos


class GzipFile(_io.BufferedIOBase):
    """The GzipFile class simulates most of the methods of a file object with
    the exception of the truncate() method.
    """

    myfileobj = None

    def __init__(
        self,
        filename=None,
        mode=None,
        compresslevel=_COMPRESS_LEVEL_BEST,
        fileobj=None,
        mtime=None,
    ):
        if mode and ("t" in mode or "U" in mode):
            raise ValueError("Invalid mode: {!r}".format(mode))
        if mode and "b" not in mode:
            mode += "b"
        if fileobj is None:
            if filename is None:
                raise TypeError("expected str, bytes or os.PathLike object, not NoneType")
            fileobj = self.myfileobj = _open_disk_file(filename, mode or "rb")
        if filename is None:
            filename = getattr(fileobj, "name", "")
            if not isinstance(filename, (str, bytes)):
                filename = ""
        else:
            filename = _os.fspath(filename)
        origmode = mode
        if mode is None:
            mode = getattr(fileobj, "mode", "rb")
            if not isinstance(mode, str):
                mode = "rb"

        if mode.startswith("r"):
            self.mode = READ
            self._reader = _GzipReader(fileobj)
            self._name = filename
        elif mode.startswith(("w", "a", "x")):
            if origmode is None:
                pass
            self.mode = WRITE
            self._init_write(filename)
            self.compress = _zlib.compressobj(
                compresslevel, _zlib.DEFLATED, -_zlib.MAX_WBITS, _zlib.DEF_MEM_LEVEL, 0
            )
            self._write_mtime = mtime
            self._compresslevel = compresslevel
            self._header_written = False
        else:
            raise ValueError("Invalid mode: {!r}".format(mode))

        self.fileobj = fileobj
        self._closed = False
        self.offset = 0

        if self.mode == WRITE:
            self._ensure_header()

    def _init_write(self, filename):
        self._name = filename
        self.crc = _zlib.crc32(b"")
        self.size = 0
        self.writebuf = []
        self.bufsize = 0

    def _ensure_header(self):
        if not self._header_written:
            fname = self._name
            if isinstance(fname, str):
                fname = _os.path.basename(fname)
            elif isinstance(fname, bytes):
                fname = _os.path.basename(fname.decode("latin-1"))
            else:
                fname = ""
            _write_gzip_header(self.fileobj, fname, self._compresslevel, self._write_mtime)
            self._header_written = True

    @property
    def name(self):
        return self._name

    @property
    def filename(self):
        return self._name

    @property
    def mtime(self):
        if self.mode == READ:
            return self._reader._last_mtime
        return self._write_mtime

    @property
    def closed(self):
        return self._closed

    def _check_not_closed(self):
        if self._closed:
            raise ValueError("I/O operation on closed file")

    def readable(self):
        self._check_not_closed()
        return self.mode == READ

    def writable(self):
        self._check_not_closed()
        return self.mode == WRITE

    def seekable(self):
        return True

    def write(self, data):
        self._check_not_closed()
        if self.mode != WRITE:
            raise _io.UnsupportedOperation("write() on read-only GzipFile object")
        raw = _to_bytes(data)
        length = len(raw)
        if length > 0:
            self._ensure_header()
            self.fileobj.write(self.compress.compress(raw))
            self.size += length
            self.crc = _zlib.crc32(raw, self.crc)
            self.offset += length
        return length

    def writelines(self, lines):
        self._check_not_closed()
        for line in lines:
            self.write(line)

    def read(self, size=-1):
        self._check_not_closed()
        if self.mode != READ:
            raise _io.UnsupportedOperation("read() on write-only GzipFile object")
        data = self._reader.read(size)
        self.offset = self._reader._pos
        return data

    def read1(self, size=-1):
        self._check_not_closed()
        if self.mode != READ:
            raise _io.UnsupportedOperation("read1() on write-only GzipFile object")
        if size is None or size < 0:
            size = _io.DEFAULT_BUFFER_SIZE
        data = self._reader.read(size)
        self.offset = self._reader._pos
        return data

    def peek(self, n):
        self._check_not_closed()
        if self.mode != READ:
            raise _io.UnsupportedOperation("peek() on write-only GzipFile object")
        return self._reader.peek(n)

    def readline(self, size=-1):
        self._check_not_closed()
        if self.mode != READ:
            raise _io.UnsupportedOperation("readline() on write-only GzipFile object")
        line = self._reader.readline(size)
        self.offset = self._reader._pos
        return line

    def readlines(self, hint=-1):
        self._check_not_closed()
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

    def __iter__(self):
        return self

    def __next__(self):
        line = self.readline()
        if not line:
            raise StopIteration
        return line

    def flush(self, zlib_mode=_zlib.Z_SYNC_FLUSH):
        self._check_not_closed()
        if self.mode == WRITE:
            self.fileobj.write(self.compress.flush(zlib_mode))
            if hasattr(self.fileobj, "flush"):
                self.fileobj.flush()

    def tell(self):
        self._check_not_closed()
        if self.mode == READ:
            return self._reader._pos
        return self.offset

    def rewind(self):
        self._check_not_closed()
        if self.mode != READ:
            raise OSError("Can't rewind in write mode")
        self._reader.rewind()
        self.offset = 0

    def seek(self, offset, whence=_io.SEEK_SET):
        self._check_not_closed()
        if self.mode == WRITE:
            if whence != _io.SEEK_SET:
                if whence == _io.SEEK_CUR:
                    offset = self.offset + offset
                else:
                    raise ValueError("Seek from end not supported")
            if offset < self.offset:
                raise OSError("Negative seek in write mode")
            count = offset - self.offset
            chunk = b"\x00" * 1024
            for _ in range(count // 1024):
                self.write(chunk)
            rem = count % 1024
            if rem:
                self.write(b"\x00" * rem)
            return self.offset
        else:
            pos = self._reader.seek(offset, whence)
            self.offset = pos
            return pos

    def close(self):
        if self._closed:
            return
        fileobj = self.fileobj
        if fileobj is None:
            self._closed = True
            return
        try:
            if self.mode == WRITE:
                self._ensure_header()
                fileobj.write(self.compress.flush(_zlib.Z_FINISH))
                fileobj.write(_struct.pack("<II", self.crc & 0xFFFFFFFF, self.size & 0xFFFFFFFF))
        finally:
            self._closed = True
            self.fileobj = None
            myfileobj = self.myfileobj
            if myfileobj is not None:
                self.myfileobj = None
                myfileobj.close()

    def __enter__(self):
        self._check_not_closed()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False

    def __repr__(self):
        s = repr(self.fileobj)
        return "<gzip " + s[1:-1] + " " + hex(id(self)) + ">"


class _TextGzipWrapper(_io.TextIOBase):
    """Text-mode wrapper around a binary ``GzipFile`` for ``gzip.open(..., 'rt'/'wt'/'at')``."""

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
            # Keep original line endings (\r\n, \r, or \n).
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
    compresslevel=_COMPRESS_LEVEL_BEST,
    encoding=None,
    errors=None,
    newline=None,
):
    """Open a gzip-compressed file in binary or text mode."""
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

    gz_mode = mode.replace("t", "")
    if isinstance(filename, (str, bytes)) or hasattr(filename, "__fspath__"):
        binary_file = GzipFile(filename, gz_mode, compresslevel)
    elif hasattr(filename, "read") or hasattr(filename, "write"):
        binary_file = GzipFile(None, gz_mode, compresslevel, filename)
    else:
        raise TypeError("filename must be a str or bytes object, or a file")

    if "t" in mode:
        return _TextGzipWrapper(
            binary_file, encoding=encoding, errors=errors, newline=newline
        )
    return binary_file


def compress(data, compresslevel=_COMPRESS_LEVEL_BEST, *, mtime=None):
    """Compress data in one shot and return the compressed string."""
    raw = _to_bytes(data)
    buf = _io.BytesIO()
    with GzipFile(fileobj=buf, mode="wb", compresslevel=compresslevel, mtime=mtime) as f:
        f.write(raw)
    return buf.getvalue()


def decompress(data):
    """Decompress a gzip compressed string in one shot."""
    raw = _to_bytes(data)
    with GzipFile(fileobj=_io.BytesIO(raw), mode="rb") as f:
        return f.read()
