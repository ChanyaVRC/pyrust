"""Read and write ZIP files.

Python 3.12 ``zipfile`` module implementation built on ``zlib``, ``struct``,
``io``, ``os``, ``time``, and ``shutil`` (issue #3186).
"""

import builtins as _builtins
import io as _io
import os as _os
import shutil as _shutil
import struct as _struct
import time as _time
import zlib as _zlib

__all__ = [
    "BadZipFile",
    "BadZipfile",
    "error",
    "ZIP_STORED",
    "ZIP_DEFLATED",
    "ZIP_BZIP2",
    "ZIP_LZMA",
    "is_zipfile",
    "ZipInfo",
    "ZipFile",
    "LargeZipFile",
]

ZIP_STORED = 0
ZIP_DEFLATED = 8
ZIP_BZIP2 = 12
ZIP_LZMA = 14

ZIP64_LIMIT = (1 << 31) - 1
ZIP_FILECOUNT_LIMIT = (1 << 16) - 1
ZIP_MAX_COMMENT = (1 << 16) - 1

_SIG_LOCAL_FILE_HEADER = b"PK\x03\x04"
_SIG_CENTRAL_DIR = b"PK\x01\x02"
_SIG_END_CENTRAL_DIR = b"PK\x05\x06"
_SIG_ZIP64_END_CENTRAL_DIR = b"PK\x06\x06"
_SIG_ZIP64_LOCATOR = b"PK\x06\x07"

_FMT_LOCAL_HEADER = "<4s2B4HL2L2H"
_SIZE_LOCAL_HEADER = 30
_FMT_CENTRAL_DIR = "<4s4B4HL2L5H2L"
_SIZE_CENTRAL_DIR = 46
_FMT_END_CENTRAL_DIR = "<4s4H2LH"
_SIZE_END_CENTRAL_DIR = 22
_FMT_ZIP64_LOCATOR = "<4sLQL"
_SIZE_ZIP64_LOCATOR = 20
_FMT_ZIP64_END_CENTRAL_DIR = "<4sQ2H2L4Q"
_SIZE_ZIP64_END_CENTRAL_DIR = 56

_MASK_UTF_FILENAME = 0x800
_MASK_ENCRYPTED = 0x1

_S_IFMT = 0o170000
_S_IFDIR = 0o040000
_S_IFREG = 0o100000


class BadZipFile(Exception):
    pass


class LargeZipFile(Exception):
    """Raised when writing a zipfile, the zipfile requires ZIP64 extensions
    and those extensions are disabled.
    """


error = BadZipfile = BadZipFile


def _fspath_str(path):
    p = _os.fspath(path)
    if isinstance(p, bytes):
        return p.decode("utf-8")
    return p


def _check_compression(compression):
    if compression == ZIP_STORED or compression == ZIP_DEFLATED:
        return
    if compression in (ZIP_BZIP2, ZIP_LZMA):
        raise NotImplementedError("That compression method is not supported")
    raise NotImplementedError("That compression method is not supported")


def _get_compressor(compress_type, compresslevel=None):
    if compress_type == ZIP_DEFLATED:
        level = _zlib.Z_DEFAULT_COMPRESSION if compresslevel is None else compresslevel
        return _zlib.compressobj(level, _zlib.DEFLATED, -_zlib.MAX_WBITS)
    if compress_type == ZIP_STORED:
        return None
    _check_compression(compress_type)
    return None


def _get_decompressor(compress_type):
    if compress_type == ZIP_STORED:
        return None
    if compress_type == ZIP_DEFLATED:
        return _zlib.decompressobj(-_zlib.MAX_WBITS)
    _check_compression(compress_type)
    return None


def _find_end_rec(fp):
    """Find and parse End of Central Directory (and ZIP64 EOCD if present).

    Returns `(num_entries, cd_size, cd_offset, comment, eocd_offset)` or `None`.
    """
    try:
        fp.seek(0, _io.SEEK_END)
        filesize = fp.tell()
    except Exception:
        return None

    if filesize < _SIZE_END_CENTRAL_DIR:
        return None

    # Fast path: no archive comment.
    fp.seek(filesize - _SIZE_END_CENTRAL_DIR)
    data = fp.read(_SIZE_END_CENTRAL_DIR)
    eocd_pos = -1
    if len(data) == _SIZE_END_CENTRAL_DIR and data[:4] == _SIG_END_CENTRAL_DIR:
        rec = _struct.unpack(_FMT_END_CENTRAL_DIR, data)
        if rec[7] == 0:
            eocd_pos = filesize - _SIZE_END_CENTRAL_DIR
            comment = b""

    if eocd_pos < 0:
        max_back = min(filesize, ZIP_MAX_COMMENT + _SIZE_END_CENTRAL_DIR)
        fp.seek(filesize - max_back)
        tail = fp.read(max_back)
        search_pos = len(tail) - _SIZE_END_CENTRAL_DIR
        while search_pos >= 0:
            idx = tail.rfind(_SIG_END_CENTRAL_DIR, 0, search_pos + 4)
            if idx < 0:
                break
            if idx + _SIZE_END_CENTRAL_DIR <= len(tail):
                rec = _struct.unpack(
                    _FMT_END_CENTRAL_DIR, tail[idx : idx + _SIZE_END_CENTRAL_DIR]
                )
                comment_len = rec[7]
                if idx + _SIZE_END_CENTRAL_DIR + comment_len == len(tail):
                    eocd_pos = filesize - max_back + idx
                    comment = tail[idx + _SIZE_END_CENTRAL_DIR :]
                    break
            search_pos = idx - 1

    if eocd_pos < 0:
        return None

    # rec: (sig, disk_no, disk_cd, disk_entries, total_entries, cd_size, cd_offset, comment_len)
    total_entries = rec[4]
    cd_size = rec[5]
    cd_offset = rec[6]

    # Check for ZIP64 End of Central Directory locator right before EOCD.
    if (
        total_entries == 0xFFFF
        or cd_size == 0xFFFFFFFF
        or cd_offset == 0xFFFFFFFF
    ) and eocd_pos >= _SIZE_ZIP64_LOCATOR:
        fp.seek(eocd_pos - _SIZE_ZIP64_LOCATOR)
        loc_data = fp.read(_SIZE_ZIP64_LOCATOR)
        if len(loc_data) == _SIZE_ZIP64_LOCATOR and loc_data[:4] == _SIG_ZIP64_LOCATOR:
            _, _, z64_offset, _ = _struct.unpack(_FMT_ZIP64_LOCATOR, loc_data)
            if 0 <= z64_offset <= filesize - _SIZE_ZIP64_END_CENTRAL_DIR:
                fp.seek(z64_offset)
                z64_data = fp.read(_SIZE_ZIP64_END_CENTRAL_DIR)
                if (
                    len(z64_data) == _SIZE_ZIP64_END_CENTRAL_DIR
                    and z64_data[:4] == _SIG_ZIP64_END_CENTRAL_DIR
                ):
                    z64 = _struct.unpack(_FMT_ZIP64_END_CENTRAL_DIR, z64_data)
                    total_entries = z64[7]
                    cd_size = z64[8]
                    cd_offset = z64[9]

    return (total_entries, cd_size, cd_offset, comment, eocd_pos)


def is_zipfile(filename):
    """Quickly see if a file is a ZIP file by checking the magic number."""
    try:
        if hasattr(filename, "read") and hasattr(filename, "seek"):
            pos = filename.tell()
            try:
                return _find_end_rec(filename) is not None
            finally:
                filename.seek(pos)
        else:
            path_s = _fspath_str(filename)
            with _builtins.open(path_s, "rb") as fp:
                return _find_end_rec(fp) is not None
    except OSError:
        return False
    except TypeError:
        return False


class ZipInfo:
    """Class with attributes describing each file in the ZIP archive."""

    def __init__(self, filename="NoName", date_time=(1980, 1, 1, 0, 0, 0)):
        self.orig_filename = filename
        null_byte = filename.find("\x00")
        if null_byte >= 0:
            filename = filename[:null_byte]
        if _os.sep != "/" and _os.sep in filename:
            filename = filename.replace(_os.sep, "/")

        self.filename = filename
        self.date_time = date_time
        if date_time[0] < 1980:
            raise ValueError("ZIP does not support timestamps before 1980")

        self.compress_type = ZIP_STORED
        self._compresslevel = None
        self.comment = b""
        self.extra = b""
        self.create_system = 0 if _os.name == "nt" else 3
        self.create_version = 20
        self.extract_version = 20
        self.reserved = 0
        self.flag_bits = 0
        self.volume = 0
        self.internal_attr = 0
        self.external_attr = 0
        self.header_offset = 0
        self.CRC = 0
        self.compress_size = 0
        self.file_size = 0

    def __repr__(self):
        parts = ["<%s filename=%r" % (self.__class__.__name__, self.filename)]
        if self.compress_type != ZIP_STORED:
            ctype = "deflate" if self.compress_type == ZIP_DEFLATED else str(self.compress_type)
            parts.append(" compress_type=%s" % ctype)
        if not self.is_dir():
            parts.append(" file_size=%r" % self.file_size)
            if self.compress_type != ZIP_STORED:
                parts.append(" compress_size=%r" % self.compress_size)
        parts.append(">")
        return "".join(parts)

    def _encodeFilenameFlags(self):
        try:
            return self.filename.encode("ascii"), self.flag_bits
        except UnicodeEncodeError:
            return self.filename.encode("utf-8"), self.flag_bits | _MASK_UTF_FILENAME

    def _decodeExtra(self, crc_size_offset=False):
        extra = self.extra
        pos = 0
        n = len(extra)
        while pos + 4 <= n:
            tp, ln = _struct.unpack("<HH", extra[pos : pos + 4])
            pos += 4
            if pos + ln > n:
                break
            field = extra[pos : pos + ln]
            pos += ln
            if tp == 0x0001:
                idx = 0
                if self.file_size == 0xFFFFFFFF and idx + 8 <= ln:
                    (self.file_size,) = _struct.unpack("<Q", field[idx : idx + 8])
                    idx += 8
                if self.compress_size == 0xFFFFFFFF and idx + 8 <= ln:
                    (self.compress_size,) = _struct.unpack("<Q", field[idx : idx + 8])
                    idx += 8
                if self.header_offset == 0xFFFFFFFF and idx + 8 <= ln:
                    (self.header_offset,) = _struct.unpack("<Q", field[idx : idx + 8])
                    idx += 8

    def FileHeader(self, zip64=None):
        dt = self.date_time
        dosdate = ((dt[0] - 1980) << 9) | (dt[1] << 5) | dt[2]
        dostime = (dt[3] << 11) | (dt[4] << 5) | (dt[5] // 2)

        file_size = self.file_size
        compress_size = self.compress_size
        extra = self.extra
        min_version = 0

        if zip64 is None:
            zip64 = file_size > ZIP64_LIMIT or compress_size > ZIP64_LIMIT
        if zip64:
            extra = extra + _struct.pack("<HHQQ", 1, 16, file_size, compress_size)
            file_size = 0xFFFFFFFF
            compress_size = 0xFFFFFFFF
            min_version = 45

        if self.compress_type == ZIP_DEFLATED:
            min_version = max(min_version, 20)
        self.extract_version = max(20, min_version)
        self.create_version = max(20, min_version)

        filename, flag_bits = self._encodeFilenameFlags()
        header = _struct.pack(
            _FMT_LOCAL_HEADER,
            _SIG_LOCAL_FILE_HEADER,
            self.extract_version,
            self.reserved,
            flag_bits,
            self.compress_type,
            dostime,
            dosdate,
            self.CRC & 0xFFFFFFFF,
            compress_size,
            file_size,
            len(filename),
            len(extra),
        )
        return header + filename + extra

    def is_dir(self):
        """Return True if this archive member is a directory."""
        return self.filename[-1:] == "/"

    @classmethod
    def from_file(cls, filename, arcname=None, *, strict_timestamps=True):
        """Construct an appropriate ZipInfo for a file on the filesystem."""
        path_s = _fspath_str(filename)
        st = _os.stat(path_s)
        isdir = (st.st_mode & _S_IFMT) == _S_IFDIR
        mtime = _time.localtime(st.st_mtime)
        date_time = mtime[0:6]
        if not strict_timestamps and date_time[0] < 1980:
            date_time = (1980, 1, 1, 0, 0, 0)
        elif not strict_timestamps and date_time[0] > 2107:
            date_time = (2107, 12, 31, 23, 59, 59)

        if arcname is None:
            arcname = path_s
        else:
            arcname = _fspath_str(arcname)
        arcname = _os.path.normpath(_os.path.splitdrive(arcname)[1])
        while arcname and arcname[0] in (_os.sep, "/", "\\"):
            arcname = arcname[1:]
        if _os.sep != "/":
            arcname = arcname.replace(_os.sep, "/")
        if isdir:
            if not arcname.endswith("/"):
                arcname += "/"

        zinfo = cls(arcname, date_time)
        zinfo.external_attr = (st.st_mode & 0xFFFF) << 16
        if isdir:
            zinfo.file_size = 0
            zinfo.external_attr |= 0x10
        else:
            zinfo.file_size = st.st_size
        return zinfo


class ZipExtFile(_io.BufferedIOBase):
    """File-like object for reading an archive member."""

    def __init__(self, raw_data, zinfo):
        self._zinfo = zinfo
        self._raw = raw_data
        self._raw_pos = 0
        self._decompressor = _get_decompressor(zinfo.compress_type)
        self._buf = bytearray()
        self._eof = False
        self._crc = 0
        self._pos = 0
        self._closed = False
        self._all_data = None

    @property
    def name(self):
        return self._zinfo.filename

    @property
    def closed(self):
        return self._closed

    def readable(self):
        return True

    def writable(self):
        return False

    def seekable(self):
        return True

    def _check_not_closed(self):
        if self._closed:
            raise ValueError("I/O operation on closed file.")

    def _pump(self, size):
        if self._eof:
            return
        if self._decompressor is None:
            # ZIP_STORED
            if size < 0:
                chunk = self._raw[self._raw_pos :]
                self._raw_pos = len(self._raw)
            else:
                need = max(size - len(self._buf), 0)
                chunk = self._raw[self._raw_pos : self._raw_pos + need]
                self._raw_pos += len(chunk)
            if chunk:
                self._crc = _zlib.crc32(chunk, self._crc)
                self._buf.extend(chunk)
            if self._raw_pos >= len(self._raw):
                self._eof = True
                if (self._crc & 0xFFFFFFFF) != (self._zinfo.CRC & 0xFFFFFFFF):
                    raise BadZipFile("Bad CRC-32 for file %r" % self._zinfo.filename)
            return

        # ZIP_DEFLATED
        while not self._eof and (size < 0 or len(self._buf) < size):
            if self._decompressor.unconsumed_tail:
                chunk = self._decompressor.unconsumed_tail
            elif self._raw_pos < len(self._raw):
                end = min(self._raw_pos + 32768, len(self._raw))
                chunk = self._raw[self._raw_pos : end]
                self._raw_pos = end
            else:
                try:
                    data = self._decompressor.flush()
                except _zlib.error as exc:
                    raise BadZipFile(str(exc)) from exc
                if data:
                    self._crc = _zlib.crc32(data, self._crc)
                    self._buf.extend(data)
                self._eof = True
                if not self._decompressor.eof:
                    raise BadZipFile(
                        "Truncated compressed stream for file %r" % self._zinfo.filename
                    )
                if (self._crc & 0xFFFFFFFF) != (self._zinfo.CRC & 0xFFFFFFFF):
                    raise BadZipFile("Bad CRC-32 for file %r" % self._zinfo.filename)
                break

            max_len = 0 if size < 0 else max(size - len(self._buf), 1)
            try:
                data = self._decompressor.decompress(chunk, max_len)
            except _zlib.error as exc:
                raise BadZipFile(str(exc)) from exc
            if data:
                self._crc = _zlib.crc32(data, self._crc)
                self._buf.extend(data)
            if self._decompressor.eof:
                self._eof = True
                if (self._crc & 0xFFFFFFFF) != (self._zinfo.CRC & 0xFFFFFFFF):
                    raise BadZipFile("Bad CRC-32 for file %r" % self._zinfo.filename)
                break

    def read(self, n=-1):
        self._check_not_closed()
        if n == 0:
            return b""
        if n is None or n < 0:
            self._pump(-1)
            out = bytes(self._buf)
            self._buf.clear()
            self._pos += len(out)
            return out
        self._pump(n)
        out = bytes(self._buf[:n])
        del self._buf[: len(out)]
        self._pos += len(out)
        return out

    def read1(self, n=-1):
        return self.read(n if (n is not None and n >= 0) else _io.DEFAULT_BUFFER_SIZE)

    def peek(self, n=1):
        self._check_not_closed()
        if not self._buf and not self._eof:
            self._pump(max(n, 1))
        return bytes(self._buf)

    def readline(self, limit=-1):
        self._check_not_closed()
        if limit is None:
            limit = -1
        if limit == 0:
            return b""
        out = bytearray()
        while True:
            if not self._buf:
                if self._eof:
                    break
                self._pump(1)
                if not self._buf:
                    break
            nl = self._buf.find(b"\n")
            if nl >= 0:
                take = nl + 1
                if limit > 0:
                    take = min(take, limit - len(out))
                out.extend(self._buf[:take])
                del self._buf[:take]
                self._pos += take
                break
            if limit > 0 and len(out) + len(self._buf) >= limit:
                take = limit - len(out)
                out.extend(self._buf[:take])
                del self._buf[:take]
                self._pos += take
                break
            take = len(self._buf)
            out.extend(self._buf)
            self._buf.clear()
            self._pos += take
        return bytes(out)

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

    def tell(self):
        self._check_not_closed()
        return self._pos

    def seek(self, offset, whence=_io.SEEK_SET):
        self._check_not_closed()
        if whence == _io.SEEK_SET:
            target = offset
        elif whence == _io.SEEK_CUR:
            target = self._pos + offset
        elif whence == _io.SEEK_END:
            while self.read(32768):
                pass
            target = self._pos + offset
        else:
            raise ValueError("whence must be os.SEEK_SET (0), os.SEEK_CUR (1), or os.SEEK_END (2)")
        if target < 0:
            target = 0
        if target < self._pos:
            # Rewind from start of raw compressed bytes.
            self._raw_pos = 0
            self._decompressor = _get_decompressor(self._zinfo.compress_type)
            self._buf.clear()
            self._eof = False
            self._crc = 0
            self._pos = 0
        skip = target - self._pos
        while skip > 0:
            chunk = self.read(min(skip, 32768))
            if not chunk:
                break
            skip -= len(chunk)
        return self._pos

    def __iter__(self):
        return self

    def __next__(self):
        line = self.readline()
        if not line:
            raise StopIteration
        return line

    def close(self):
        self._closed = True

    def __enter__(self):
        self._check_not_closed()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False


class _ZipWriteFile(_io.BufferedIOBase):
    """Writable file-like object returned by ``ZipFile.open(name, 'w')``."""

    def __init__(self, zf, zinfo, force_zip64=False):
        self._zf = zf
        self._zinfo = zinfo
        self._force_zip64 = force_zip64
        self._buf = bytearray()
        self._closed = False

    @property
    def name(self):
        return self._zinfo.filename

    @property
    def closed(self):
        return self._closed

    def readable(self):
        return False

    def writable(self):
        return True

    def seekable(self):
        return False

    def write(self, data):
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if isinstance(data, bytes):
            raw = data
        elif isinstance(data, bytearray):
            raw = bytes(data)
        else:
            raw = bytes(data)
        self._buf.extend(raw)
        return len(raw)

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def flush(self):
        if self._closed:
            raise ValueError("I/O operation on closed file.")

    def close(self):
        if self._closed:
            return
        self._closed = True
        self._zf.writestr(self._zinfo, bytes(self._buf))

    def __enter__(self):
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False


class ZipFile:
    """Class with methods to open, read, write, close, list zip files."""

    fp = None

    def __init__(
        self,
        file,
        mode="r",
        compression=ZIP_STORED,
        allowZip64=True,
        compresslevel=None,
        *,
        strict_timestamps=True,
        metadata_encoding=None,
    ):
        if mode not in ("r", "w", "x", "a"):
            raise ValueError("ZipFile requires mode 'r', 'w', 'x', or 'a'")

        _check_compression(compression)

        self._allowZip64 = allowZip64
        self._didModify = False
        self.debug = 0
        self.NameToInfo = {}
        self.filelist = []
        self.compression = compression
        self.compresslevel = compresslevel
        self.mode = mode
        self.pwd = None
        self._comment = b""
        self._strict_timestamps = strict_timestamps
        self.metadata_encoding = metadata_encoding
        self._disk_writeback_path = None

        if metadata_encoding and mode != "r":
            raise ValueError("metadata_encoding is only supported for reading files")

        if isinstance(file, (str, bytes)) or hasattr(file, "__fspath__"):
            self._filePassed = 0
            path_s = _fspath_str(file)
            self.filename = path_s
            if mode == "r":
                self.fp = _builtins.open(path_s, "rb")
            elif mode == "w":
                self.fp = _builtins.open(path_s, "wb")
            elif mode == "x":
                if _os.path.exists(path_s):
                    raise FileExistsError("File exists: %r" % path_s)
                self.fp = _builtins.open(path_s, "wb")
            elif mode == "a":
                # Buffer existing contents in BytesIO so we can read EOCD, seek to CD start,
                # append new entries, rewrite CD+EOCD, and persist on close().
                existing = b""
                if _os.path.exists(path_s):
                    with _builtins.open(path_s, "rb") as rf:
                        existing = rf.read()
                self.fp = _io.BytesIO(existing)
                self._disk_writeback_path = path_s
        else:
            self._filePassed = 1
            self.fp = file
            self.filename = getattr(file, "name", None)

        self._fileRefCnt = 1

        try:
            if mode == "r":
                self._RealGetContents()
            elif mode in ("w", "x"):
                self._didModify = True
                try:
                    self.start_dir = self.fp.tell()
                except Exception:
                    self.start_dir = 0
            elif mode == "a":
                try:
                    self._RealGetContents()
                    self.fp.seek(self.start_dir)
                    if hasattr(self.fp, "truncate"):
                        self.fp.truncate(self.start_dir)
                except BadZipFile:
                    # Empty or non-zip file opened in 'a' mode starts fresh at EOF.
                    self.fp.seek(0, _io.SEEK_END)
                    self.start_dir = self.fp.tell()
                    self._didModify = True
        except Exception:
            fp = self.fp
            self.fp = None
            self._fpclose(fp)
            raise

    def _fpclose(self, fp):
        if fp is not None and not self._filePassed:
            fp.close()

    def _RealGetContents(self):
        fp = self.fp
        endrec = _find_end_rec(fp)
        if endrec is None:
            raise BadZipFile("File is not a zip file")

        total_entries, size_cd, offset_cd, comment, eocd_pos = endrec
        self._comment = comment

        # Compute concat offset (for self-extracting archives or prepended bytes).
        concat = eocd_pos - size_cd - offset_cd
        if concat < 0:
            concat = 0
        self.start_dir = offset_cd + concat

        if self.start_dir + size_cd > eocd_pos:
            raise BadZipFile("Truncated central directory")

        fp.seek(self.start_dir)
        data = fp.read(size_cd)
        if len(data) != size_cd:
            raise BadZipFile("Truncated central directory")

        pos = 0
        while pos < size_cd:
            if pos + _SIZE_CENTRAL_DIR > size_cd:
                raise BadZipFile("Truncated central directory entry")
            centdir = _struct.unpack(
                _FMT_CENTRAL_DIR, data[pos : pos + _SIZE_CENTRAL_DIR]
            )
            if centdir[0] != _SIG_CENTRAL_DIR:
                raise BadZipFile("Bad magic number for central directory")
            pos += _SIZE_CENTRAL_DIR

            (
                _sig,
                create_version,
                create_system,
                extract_version,
                reserved,
                flag_bits,
                compress_type,
                dostime,
                dosdate,
                crc,
                compress_size,
                file_size,
                fname_len,
                extra_len,
                comment_len,
                volume,
                internal_attr,
                external_attr,
                header_offset,
            ) = centdir

            if pos + fname_len + extra_len + comment_len > size_cd:
                raise BadZipFile("Truncated central directory entry")

            filename_bytes = data[pos : pos + fname_len]
            pos += fname_len
            extra = data[pos : pos + extra_len]
            pos += extra_len
            entry_comment = data[pos : pos + comment_len]
            pos += comment_len

            if flag_bits & _MASK_UTF_FILENAME:
                filename = filename_bytes.decode("utf-8")
            else:
                enc = self.metadata_encoding or "latin-1"
                filename = filename_bytes.decode(enc)

            year = ((dosdate >> 9) & 0x7F) + 1980
            month = (dosdate >> 5) & 0x0F
            day = dosdate & 0x1F
            hour = (dostime >> 11) & 0x1F
            minute = (dostime >> 5) & 0x3F
            second = (dostime & 0x1F) * 2

            x = ZipInfo(filename, (year, month, day, hour, minute, second))
            x.create_version = create_version
            x.create_system = create_system
            x.extract_version = extract_version
            x.reserved = reserved
            x.flag_bits = flag_bits
            x.compress_type = compress_type
            x.CRC = crc
            x.compress_size = compress_size
            x.file_size = file_size
            x.extra = extra
            x.comment = entry_comment
            x.volume = volume
            x.internal_attr = internal_attr
            x.external_attr = external_attr
            x.header_offset = header_offset + concat
            x._decodeExtra()

            self.filelist.append(x)
            self.NameToInfo[x.filename] = x

    @property
    def comment(self):
        return self._comment

    @comment.setter
    def comment(self, value):
        if not isinstance(value, bytes):
            raise TypeError(
                "comment: expected bytes, got %s" % type(value).__name__
            )
        if len(value) > ZIP_MAX_COMMENT:
            value = value[:ZIP_MAX_COMMENT]
        self._comment = value
        self._didModify = True

    def namelist(self):
        """Return a list of file names in the archive."""
        return [data.filename for data in self.filelist]

    def infolist(self):
        """Return a list of class ZipInfo instances for files in the archive."""
        return list(self.filelist)

    def getinfo(self, name):
        """Return the instance of ZipInfo given 'name'."""
        info = self.NameToInfo.get(name)
        if info is None:
            raise KeyError("There is no item named %r in the archive" % name)
        return info

    def _open_read(self, zinfo, pwd=None):
        if zinfo.flag_bits & _MASK_ENCRYPTED:
            raise NotImplementedError("Encrypted ZIP entries are not supported")
        fp = self.fp
        fp.seek(zinfo.header_offset)
        fheader = fp.read(_SIZE_LOCAL_HEADER)
        if len(fheader) != _SIZE_LOCAL_HEADER:
            raise BadZipFile("Truncated file header")
        hdr = _struct.unpack(_FMT_LOCAL_HEADER, fheader)
        if hdr[0] != _SIG_LOCAL_FILE_HEADER:
            raise BadZipFile("Bad magic number for file header")
        fname_len = hdr[10]
        extra_len = hdr[11]
        fp.seek(zinfo.header_offset + _SIZE_LOCAL_HEADER + fname_len + extra_len)
        raw_data = fp.read(zinfo.compress_size)
        if len(raw_data) != zinfo.compress_size:
            raise BadZipFile("Truncated compressed data for %r" % zinfo.filename)
        return ZipExtFile(raw_data, zinfo)

    def open(self, name, mode="r", pwd=None, *, force_zip64=False):
        """Return file-like object for 'name'."""
        if mode not in ("r", "w"):
            raise ValueError('open() requires mode "r" or "w"')
        if not self.fp:
            raise ValueError("Attempt to use ZIP archive that was already closed")

        if mode == "w":
            if self.mode == "r":
                raise ValueError('open() for writing requires mode "w", "x", or "a"')
            if isinstance(name, ZipInfo):
                zinfo = name
            else:
                zinfo = ZipInfo(filename=name, date_time=_time.localtime(_time.time())[:6])
                zinfo.compress_type = self.compression
                zinfo._compresslevel = self.compresslevel
            return _ZipWriteFile(self, zinfo, force_zip64=force_zip64)

        if isinstance(name, ZipInfo):
            zinfo = name
        else:
            zinfo = self.getinfo(name)
        return self._open_read(zinfo, pwd=pwd)

    def read(self, name, pwd=None):
        """Return file bytes for name."""
        with self.open(name, "r", pwd) as fp:
            return fp.read()

    def testzip(self):
        """Read all the files and check the CRC."""
        chunk_size = 1 << 16
        for zinfo in self.filelist:
            try:
                with self.open(zinfo, "r") as f:
                    while f.read(chunk_size):
                        pass
            except BadZipFile:
                return zinfo.filename
        return None

    def _writecheck(self, zinfo):
        if not self.fp:
            raise ValueError("Attempt to write to ZIP archive that was already closed")
        if self.mode == "r":
            raise ValueError("Attempt to write to ZIP archive opened in read mode")
        _check_compression(zinfo.compress_type)
        if not self._allowZip64:
            if zinfo.file_size > ZIP64_LIMIT:
                raise LargeZipFile("Filesize would require ZIP64 extensions")
            if zinfo.header_offset > ZIP64_LIMIT:
                raise LargeZipFile("Zipfile size would require ZIP64 extensions")

    def writestr(self, zinfo_or_arcname, data, compress_type=None, compresslevel=None):
        """Write a file into the archive. The contents are 'data', which may be
        either a 'str' or a 'bytes' instance.
        """
        if isinstance(data, str):
            data = data.encode("utf-8")
        elif isinstance(data, bytearray):
            data = bytes(data)
        elif not isinstance(data, bytes):
            data = bytes(data)

        if not isinstance(zinfo_or_arcname, ZipInfo):
            zinfo = ZipInfo(
                filename=zinfo_or_arcname,
                date_time=_time.localtime(_time.time())[:6],
            )
            zinfo.compress_type = self.compression
            zinfo._compresslevel = self.compresslevel
            if zinfo.filename.endswith("/"):
                zinfo.external_attr = 0o40775 << 16
                zinfo.external_attr |= 0x10
            else:
                zinfo.external_attr = 0o600 << 16
        else:
            zinfo = zinfo_or_arcname

        if compress_type is not None:
            zinfo.compress_type = compress_type
        if compresslevel is not None:
            zinfo._compresslevel = compresslevel

        self._writecheck(zinfo)
        self._didModify = True

        zinfo.file_size = len(data)
        zinfo.CRC = _zlib.crc32(data) & 0xFFFFFFFF

        cmpr = _get_compressor(zinfo.compress_type, zinfo._compresslevel)
        if cmpr is not None:
            compressed = cmpr.compress(data) + cmpr.flush()
        else:
            compressed = data

        zinfo.compress_size = len(compressed)
        try:
            zinfo.header_offset = self.fp.tell()
        except Exception:
            zinfo.header_offset = self.start_dir

        use_zip64 = zinfo.file_size > ZIP64_LIMIT or zinfo.compress_size > ZIP64_LIMIT
        if use_zip64 and not self._allowZip64:
            raise LargeZipFile("Filesize would require ZIP64 extensions")

        header = zinfo.FileHeader(zip64=use_zip64)
        self.fp.write(header)
        self.fp.write(compressed)
        self.start_dir = zinfo.header_offset + len(header) + len(compressed)

        self.filelist.append(zinfo)
        self.NameToInfo[zinfo.filename] = zinfo

    def write(self, filename, arcname=None, compress_type=None, compresslevel=None):
        """Put the bytes from filename into the archive under the name arcname."""
        if not self.fp:
            raise ValueError("Attempt to write to ZIP archive that was already closed")
        if self.mode == "r":
            raise ValueError("Attempt to write to ZIP archive opened in read mode")

        zinfo = ZipInfo.from_file(
            filename, arcname, strict_timestamps=self._strict_timestamps
        )
        if zinfo.is_dir():
            zinfo.compress_type = ZIP_STORED
            self.writestr(zinfo, b"")
        else:
            zinfo.compress_type = (
                self.compression if compress_type is None else compress_type
            )
            zinfo._compresslevel = (
                self.compresslevel if compresslevel is None else compresslevel
            )
            with _builtins.open(_fspath_str(filename), "rb") as src:
                data = src.read()
            self.writestr(zinfo, data, zinfo.compress_type, zinfo._compresslevel)

    def mkdir(self, zinfo_or_directory_name, mode=511):
        """Creates a directory inside the zip archive."""
        if isinstance(zinfo_or_directory_name, ZipInfo):
            zinfo = zinfo_or_directory_name
            if not zinfo.is_dir():
                raise ValueError("The given ZipInfo does not describe a directory")
        else:
            dname = zinfo_or_directory_name.rstrip("/") + "/"
            zinfo = ZipInfo(dname, _time.localtime(_time.time())[:6])
            zinfo.compress_size = 0
            zinfo.CRC = 0
            zinfo.external_attr = ((0o040000 | (mode & 0o7777)) << 16) | 0x10
            zinfo.file_size = 0
        self.writestr(zinfo, b"", compress_type=ZIP_STORED)

    @classmethod
    def _sanitize_windows_name(cls, arcname, pathsep):
        """Replace bad characters and remove trailing dots from parts."""
        illegal = ':<>|"?*'
        out_parts = []
        for part in arcname.split(pathsep):
            cleaned = []
            for ch in part:
                if ch in illegal or ord(ch) < 32:
                    cleaned.append("_")
                else:
                    cleaned.append(ch)
            part = "".join(cleaned).rstrip(". ")
            if part:
                out_parts.append(part)
        return pathsep.join(out_parts)

    def _extract_member(self, member, targetpath, pwd):
        if not isinstance(member, ZipInfo):
            member = self.getinfo(member)

        arcname = member.filename.replace("/", _os.path.sep)
        if _os.path.altsep:
            arcname = arcname.replace(_os.path.altsep, _os.path.sep)
        arcname = _os.path.splitdrive(arcname)[1]

        invalid_path_parts = ("", _os.curdir, _os.pardir)
        arcname = _os.path.sep.join(
            x for x in arcname.split(_os.path.sep) if x not in invalid_path_parts
        )
        if _os.path.sep == "\\":
            arcname = self._sanitize_windows_name(arcname, _os.path.sep)

        targetpath = _os.path.join(targetpath, arcname)
        targetpath = _os.path.normpath(targetpath)

        upperdirs = _os.path.dirname(targetpath)
        if upperdirs and not _os.path.exists(upperdirs):
            _os.makedirs(upperdirs, exist_ok=True)

        if member.is_dir():
            if not _os.path.isdir(targetpath):
                _os.makedirs(targetpath, exist_ok=True)
            return targetpath

        with self.open(member, "r", pwd) as source, _builtins.open(targetpath, "wb") as target:
            _shutil.copyfileobj(source, target)

        return targetpath

    def extract(self, member, path=None, pwd=None):
        """Extract a member from the archive to the current working directory."""
        if path is None:
            path = _os.getcwd()
        else:
            path = _fspath_str(path)
        return self._extract_member(member, path, pwd)

    def extractall(self, path=None, members=None, pwd=None):
        """Extract all members from the archive to the current working directory."""
        if members is None:
            members = self.namelist()
        if path is None:
            path = _os.getcwd()
        else:
            path = _fspath_str(path)
        for zipinfo in members:
            self._extract_member(zipinfo, path, pwd)

    def _write_end_record(self):
        try:
            pos1 = self.fp.tell()
        except Exception:
            pos1 = self.start_dir

        for zinfo in self.filelist:
            dt = zinfo.date_time
            dosdate = ((dt[0] - 1980) << 9) | (dt[1] << 5) | dt[2]
            dostime = (dt[3] << 11) | (dt[4] << 5) | (dt[5] // 2)

            extra = []
            if zinfo.file_size > ZIP64_LIMIT or zinfo.compress_size > ZIP64_LIMIT:
                extra.append(zinfo.file_size)
                extra.append(zinfo.compress_size)
                file_size = 0xFFFFFFFF
                compress_size = 0xFFFFFFFF
            else:
                file_size = zinfo.file_size
                compress_size = zinfo.compress_size

            if zinfo.header_offset > ZIP64_LIMIT:
                extra.append(zinfo.header_offset)
                header_offset = 0xFFFFFFFF
            else:
                header_offset = zinfo.header_offset

            extra_data = zinfo.extra
            min_version = 0
            if extra:
                if not self._allowZip64:
                    raise LargeZipFile("Filesize would require ZIP64 extensions")
                extra_data = extra_data + _struct.pack(
                    "<HH" + "Q" * len(extra), 1, 8 * len(extra), *extra
                )
                min_version = 45

            extract_version = max(min_version, zinfo.extract_version)
            create_version = max(min_version, zinfo.create_version)
            filename, flag_bits = zinfo._encodeFilenameFlags()

            centdir = _struct.pack(
                _FMT_CENTRAL_DIR,
                _SIG_CENTRAL_DIR,
                create_version,
                zinfo.create_system,
                extract_version,
                zinfo.reserved,
                flag_bits,
                zinfo.compress_type,
                dostime,
                dosdate,
                zinfo.CRC & 0xFFFFFFFF,
                compress_size,
                file_size,
                len(filename),
                len(extra_data),
                len(zinfo.comment),
                0,
                zinfo.internal_attr,
                zinfo.external_attr,
                header_offset,
            )
            self.fp.write(centdir)
            self.fp.write(filename)
            self.fp.write(extra_data)
            self.fp.write(zinfo.comment)

        try:
            pos2 = self.fp.tell()
        except Exception:
            pos2 = pos1

        centDirCount = len(self.filelist)
        centDirSize = pos2 - pos1
        centDirOffset = pos1

        if (
            centDirCount >= ZIP_FILECOUNT_LIMIT
            or centDirOffset > ZIP64_LIMIT
            or centDirSize > ZIP64_LIMIT
        ):
            if not self._allowZip64:
                raise LargeZipFile("Zipfile requires ZIP64 extensions")
            zip64endrec = _struct.pack(
                _FMT_ZIP64_END_CENTRAL_DIR,
                _SIG_ZIP64_END_CENTRAL_DIR,
                44,
                45,
                45,
                0,
                0,
                centDirCount,
                centDirCount,
                centDirSize,
                centDirOffset,
            )
            self.fp.write(zip64endrec)
            zip64locrec = _struct.pack(
                _FMT_ZIP64_LOCATOR, _SIG_ZIP64_LOCATOR, 0, pos2, 1
            )
            self.fp.write(zip64locrec)
            centDirCount = min(centDirCount, 0xFFFF)
            centDirSize = min(centDirSize, 0xFFFFFFFF)
            centDirOffset = min(centDirOffset, 0xFFFFFFFF)

        endrec = _struct.pack(
            _FMT_END_CENTRAL_DIR,
            _SIG_END_CENTRAL_DIR,
            0,
            0,
            centDirCount,
            centDirCount,
            centDirSize,
            centDirOffset,
            len(self._comment),
        )
        self.fp.write(endrec)
        self.fp.write(self._comment)
        if hasattr(self.fp, "flush"):
            self.fp.flush()

    def close(self):
        """Close the file, and for mode 'w', 'x' and 'a' write the ending records."""
        if self.fp is None:
            return
        fp = self.fp
        try:
            if self.mode in ("w", "x", "a") and self._didModify:
                self._write_end_record()
                if self._disk_writeback_path is not None:
                    data = fp.getvalue()
                    with _builtins.open(self._disk_writeback_path, "wb") as wf:
                        wf.write(data)
        finally:
            self.fp = None
            self._disk_writeback_path = None
            self._fpclose(fp)

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False
