"""Read from and write to tar format archives.

Python 3.12 ``tarfile`` module implementation built on ``gzip``, ``zlib``,
``io``, ``os``, ``struct``, ``time``, and ``shutil`` (issue #3186).
"""

import builtins as _builtins
import copy as _copy
import gzip as _gzip
import io as _io
import os as _os
import shutil as _shutil
import struct as _struct
import time as _time

__all__ = [
    "TarFile",
    "TarInfo",
    "is_tarfile",
    "TarError",
    "ReadError",
    "CompressionError",
    "StreamError",
    "ExtractError",
    "HeaderError",
    "ENCODING",
    "USTAR_FORMAT",
    "GNU_FORMAT",
    "PAX_FORMAT",
    "DEFAULT_FORMAT",
    "open",
    "fully_trusted_filter",
    "data_filter",
    "tar_filter",
    "FilterError",
    "AbsoluteLinkError",
    "OutsideDestinationError",
    "SpecialFileError",
    "AbsolutePathError",
    "LinkOutsideDestinationError",
    "REGTYPE",
    "AREGTYPE",
    "LNKTYPE",
    "SYMTYPE",
    "CHRTYPE",
    "BLKTYPE",
    "DIRTYPE",
    "FIFOTYPE",
    "CONTTYPE",
]

NUL = b"\x00"
BLOCKSIZE = 512
RECORDSIZE = BLOCKSIZE * 20

POSIX_MAGIC = b"ustar\x0000"
GNU_MAGIC = b"ustar  \x00"

LENGTH_NAME = 100
LENGTH_LINK = 100
LENGTH_PREFIX = 155

REGTYPE = b"0"
AREGTYPE = b"\x00"
LNKTYPE = b"1"
SYMTYPE = b"2"
CHRTYPE = b"3"
BLKTYPE = b"4"
DIRTYPE = b"5"
FIFOTYPE = b"6"
CONTTYPE = b"7"

GNUTYPE_LONGNAME = b"L"
GNUTYPE_LONGLINK = b"K"
XHDTYPE = b"x"
XGLTYPE = b"g"

USTAR_FORMAT = 0
GNU_FORMAT = 1
PAX_FORMAT = 2
DEFAULT_FORMAT = PAX_FORMAT

ENCODING = "utf-8"

_S_IFMT = 0o170000
_S_IFDIR = 0o040000
_S_IFCHR = 0o020000
_S_IFBLK = 0o060000
_S_IFREG = 0o100000
_S_IFIFO = 0o010000
_S_IFLNK = 0o120000


class TarError(Exception):
    """Base exception."""


class ExtractError(TarError):
    """General exception for extract errors."""


class ReadError(TarError):
    """Exception for unreadable tar archives."""


class CompressionError(TarError):
    """Exception for unavailable compression methods."""


class StreamError(TarError):
    """Exception for unsupported operations on stream-like TarFiles."""


class HeaderError(TarError):
    """Base exception for header errors."""


class EmptyHeaderError(HeaderError):
    """Exception for empty headers."""


class TruncatedHeaderError(HeaderError):
    """Exception for truncated headers."""


class EOFHeaderError(HeaderError):
    """Exception for end of file headers."""


class InvalidHeaderError(HeaderError):
    """Exception for invalid headers."""


class SubsequentHeaderError(HeaderError):
    """Exception for missing and invalid extended headers."""


class FilterError(TarError):
    pass


class AbsolutePathError(FilterError):
    def __init__(self, tarinfo):
        self.tarinfo = tarinfo
        super().__init__(f"member {tarinfo.name!r} has an absolute path")


class OutsideDestinationError(FilterError):
    def __init__(self, tarinfo, path):
        self.tarinfo = tarinfo
        self._path = path
        super().__init__(
            f"{tarinfo.name!r} would be extracted to {path!r}, which is outside the destination"
        )


class SpecialFileError(FilterError):
    def __init__(self, tarinfo):
        self.tarinfo = tarinfo
        super().__init__(f"{tarinfo.name!r} is a special file")


class AbsoluteLinkError(FilterError):
    def __init__(self, tarinfo):
        self.tarinfo = tarinfo
        super().__init__(f"{tarinfo.name!r} is a link to an absolute path")


class LinkOutsideDestinationError(FilterError):
    def __init__(self, tarinfo, path):
        self.tarinfo = tarinfo
        self._path = path
        super().__init__(
            f"{tarinfo.name!r} would link to {path!r}, which is outside the destination"
        )


def _fspath_str(path):
    p = _os.fspath(path)
    if isinstance(p, bytes):
        return p.decode("utf-8")
    return p


def _stn(s, length, encoding, errors):
    """Convert a string to a null-terminated bytes object of exact length."""
    if isinstance(s, str):
        b = s.encode(encoding, errors)
    else:
        b = bytes(s)
    return b[:length] + (length - len(b)) * NUL


def _nts(b, encoding, errors):
    """Convert a null-terminated bytes object to a string."""
    p = b.find(NUL)
    if p != -1:
        b = b[:p]
    return b.decode(encoding, errors)


def _nti(b):
    """Convert a number field (octal or GNU base-256) to a Python int."""
    if b and b[0] in (0x80, 0xFF):
        n = 0
        for byte in b[1:]:
            n = (n << 8) + byte
        if b[0] == 0xFF:
            n -= 1 << (8 * (len(b) - 1))
        return n
    s = _nts(b, "ascii", "strict").strip()
    if not s:
        return 0
    try:
        return int(s, 8)
    except ValueError as exc:
        raise InvalidHeaderError("invalid number field") from exc


def _itn(n, digits=8, format=DEFAULT_FORMAT):
    """Convert a Python int to an octal or GNU base-256 bytes field."""
    n = int(n)
    if 0 <= n < (1 << (3 * (digits - 1))):
        s = ("%0*o" % (digits - 1, n)).encode("ascii") + NUL
        return s
    if format == GNU_FORMAT:
        if n >= 0:
            prefix = bytes([0x80])
            val = n
        else:
            prefix = bytes([0xFF])
            val = (1 << (8 * (digits - 1))) + n
        out = bytearray()
        for _ in range(digits - 1):
            out.append(val & 0xFF)
            val >>= 8
        out.reverse()
        return prefix + bytes(out)
    raise ValueError("overflow in number field")


def _calc_chksums(buf):
    """Calculate unsigned and signed checksums for a 512-byte tar header."""
    unsigned_sum = 256 + sum(buf[:148]) + sum(buf[156:512])
    signed_sum = 256
    for byte in buf[:148]:
        signed_sum += byte if byte < 128 else byte - 256
    for byte in buf[156:512]:
        signed_sum += byte if byte < 128 else byte - 256
    return unsigned_sum, signed_sum


def _create_header(info, format, encoding, errors):
    """Create a 512-byte tar header block from an info dict."""
    parts = [
        _stn(info.get("name", ""), 100, encoding, errors),
        _itn(info.get("mode", 0) & 0o7777, 8, format),
        _itn(info.get("uid", 0), 8, format),
        _itn(info.get("gid", 0), 8, format),
        _itn(info.get("size", 0), 12, format),
        _itn(info.get("mtime", 0), 12, format),
        b"        ",  # 8-byte checksum placeholder
        info.get("type", REGTYPE),
        _stn(info.get("linkname", ""), 100, encoding, errors),
        info.get("magic", POSIX_MAGIC),
        _stn(info.get("uname", ""), 32, encoding, errors),
        _stn(info.get("gname", ""), 32, encoding, errors),
        _itn(info.get("devmajor", 0), 8, format),
        _itn(info.get("devminor", 0), 8, format),
        _stn(info.get("prefix", ""), 155, encoding, errors),
    ]
    buf = _struct.pack("%ds" % BLOCKSIZE, b"".join(parts))
    chksum = _calc_chksums(buf)[0]
    buf = buf[:148] + ("%06o\x00 " % chksum).encode("ascii") + buf[156:]
    return buf


def _create_pax_header(pax_headers, header_type=XHDTYPE):
    """Build a PAX extended header block (`x` or `g`) + padded payload."""
    records = bytearray()
    for keyword, value in pax_headers.items():
        kw_b = keyword.encode("utf-8") if isinstance(keyword, str) else bytes(keyword)
        val_b = str(value).encode("utf-8") if not isinstance(value, bytes) else value
        base = b" " + kw_b + b"=" + val_b + b"\n"
        length = len(base) + 1
        while True:
            rec = str(length).encode("ascii") + base
            if len(rec) == length:
                break
            length = len(rec)
        records.extend(rec)

    payload = bytes(records)
    info = {
        "name": "PaxHeader",
        "mode": 0o644,
        "uid": 0,
        "gid": 0,
        "size": len(payload),
        "mtime": 0,
        "type": header_type,
        "magic": POSIX_MAGIC,
    }
    hdr = _create_header(info, USTAR_FORMAT, "ascii", "strict")
    rem = len(payload) % BLOCKSIZE
    if rem:
        payload += NUL * (BLOCKSIZE - rem)
    return hdr + payload


def _create_gnu_long_header(name_bytes, header_type):
    payload = name_bytes + NUL
    info = {
        "name": "././@LongLink",
        "mode": 0,
        "uid": 0,
        "gid": 0,
        "size": len(payload),
        "mtime": 0,
        "type": header_type,
        "magic": GNU_MAGIC,
    }
    hdr = _create_header(info, GNU_FORMAT, "ascii", "strict")
    rem = len(payload) % BLOCKSIZE
    if rem:
        payload += NUL * (BLOCKSIZE - rem)
    return hdr + payload


def _parse_pax_records(data):
    """Parse PAX key=value records from raw bytes into a dict[str, str]."""
    headers = {}
    pos = 0
    n = len(data)
    while pos < n:
        sp = data.find(b" ", pos)
        if sp < 0:
            break
        try:
            rec_len = int(data[pos:sp].decode("ascii"))
        except ValueError:
            break
        if rec_len <= 0 or pos + rec_len > n:
            break
        rec = data[sp + 1 : pos + rec_len]
        if rec.endswith(b"\n"):
            rec = rec[:-1]
        eq = rec.find(b"=")
        if eq >= 0:
            key = rec[:eq].decode("utf-8", "replace")
            val = rec[eq + 1 :].decode("utf-8", "replace")
            headers[key] = val
        pos += rec_len
    return headers


def _get_filtered_attrs(member, dest_path, for_data=True):
    new_attrs = {}
    name = member.name
    dest_abs = _os.path.realpath(dest_path)

    # Reject absolute paths or paths escaping dest_path.
    if name.startswith("/") or (len(name) >= 2 and name[1] == ":"):
        raise AbsolutePathError(member)
    norm_name = _os.path.normpath(name)
    if norm_name.startswith("..") or _os.path.isabs(norm_name):
        raise OutsideDestinationError(member, _os.path.join(dest_abs, norm_name))

    target = _os.path.realpath(_os.path.join(dest_abs, name))
    dest_prefix = dest_abs.rstrip(_os.sep) + _os.sep
    if target != dest_abs and not target.startswith(dest_prefix):
        raise OutsideDestinationError(member, target)

    if member.isdev() or member.isfifo():
        if for_data:
            raise SpecialFileError(member)

    if member.issym() or member.islnk():
        linkname = member.linkname
        if linkname.startswith("/") or (len(linkname) >= 2 and linkname[1] == ":"):
            raise AbsoluteLinkError(member)
        if member.issym():
            link_target = _os.path.realpath(
                _os.path.join(_os.path.dirname(target), linkname)
            )
        else:
            link_target = _os.path.realpath(_os.path.join(dest_abs, linkname))
        if link_target != dest_abs and not link_target.startswith(dest_prefix):
            raise LinkOutsideDestinationError(member, link_target)

    if for_data:
        if member.mode is not None:
            mode = member.mode & 0o777
            if member.isdir():
                mode |= 0o700
            else:
                mode = (mode & ~0o022) | 0o600
            if mode != member.mode:
                new_attrs["mode"] = mode
        new_attrs["uid"] = None
        new_attrs["gid"] = None
        new_attrs["uname"] = None
        new_attrs["gname"] = None
    return new_attrs


def fully_trusted_filter(member, dest_path):
    return member


def tar_filter(member, dest_path):
    new_attrs = _get_filtered_attrs(member, dest_path, for_data=False)
    if new_attrs:
        return member.replace(**new_attrs, deep=False)
    return member


def data_filter(member, dest_path):
    new_attrs = _get_filtered_attrs(member, dest_path, for_data=True)
    if new_attrs:
        return member.replace(**new_attrs, deep=False)
    return member


_NAMED_FILTERS = {
    "fully_trusted": fully_trusted_filter,
    "tar": tar_filter,
    "data": data_filter,
}


class TarInfo:
    """Informational class which holds the details about an archive member."""

    def __init__(self, name=""):
        self.name = name
        self.mode = 0o644
        self.uid = 0
        self.gid = 0
        self.size = 0
        self.mtime = 0
        self.chksum = 0
        self.type = REGTYPE
        self.linkname = ""
        self.uname = ""
        self.gname = ""
        self.devmajor = 0
        self.devminor = 0
        self.offset = 0
        self.offset_data = 0
        self.pax_headers = {}

    def __repr__(self):
        return "<%s %r at %#x>" % (self.__class__.__name__, self.name, id(self))

    def get_info(self):
        return {
            "name": self.name,
            "mode": self.mode,
            "uid": self.uid,
            "gid": self.gid,
            "size": self.size,
            "mtime": self.mtime,
            "chksum": self.chksum,
            "type": self.type,
            "linkname": self.linkname,
            "uname": self.uname,
            "gname": self.gname,
            "devmajor": self.devmajor,
            "devminor": self.devminor,
        }

    def replace(
        self,
        *,
        name=None,
        mtime=None,
        mode=None,
        linkname=None,
        uid=None,
        gid=None,
        uname=None,
        gname=None,
        deep=True,
    ):
        result = _copy.deepcopy(self) if deep else _copy.copy(self)
        if name is not None:
            result.name = name
        if mtime is not None:
            result.mtime = mtime
        if mode is not None:
            result.mode = mode
        if linkname is not None:
            result.linkname = linkname
        if uid is not None:
            result.uid = uid
        if gid is not None:
            result.gid = gid
        if uname is not None:
            result.uname = uname
        if gname is not None:
            result.gname = gname
        return result

    def isfile(self):
        return self.isreg()

    def isreg(self):
        return self.type in (REGTYPE, AREGTYPE)

    def isdir(self):
        return self.type == DIRTYPE

    def issym(self):
        return self.type == SYMTYPE

    def islnk(self):
        return self.type == LNKTYPE

    def ischr(self):
        return self.type == CHRTYPE

    def isblk(self):
        return self.type == BLKTYPE

    def isfifo(self):
        return self.type == FIFOTYPE

    def isdev(self):
        return self.type in (CHRTYPE, BLKTYPE, FIFOTYPE)

    def issparse(self):
        return False

    def tobuf(self, format=DEFAULT_FORMAT, encoding=ENCODING, errors="surrogateescape"):
        info = self.get_info()
        name = info["name"]
        if self.isdir() and name and not name.endswith("/"):
            name += "/"
        info["name"] = name

        if format == USTAR_FORMAT:
            return self._create_ustar(info, encoding, errors)
        if format == GNU_FORMAT:
            return self._create_gnu(info, encoding, errors)
        if format == PAX_FORMAT:
            return self._create_pax(info, encoding, errors)
        raise ValueError("invalid format")

    def _create_ustar(self, info, encoding, errors):
        info["magic"] = POSIX_MAGIC
        name_b = info["name"].encode(encoding, errors)
        if len(name_b) > LENGTH_NAME:
            # Try splitting at '/' into prefix (<=155) and name (<=100).
            split_pos = info["name"].rfind("/", 0, LENGTH_PREFIX + 1)
            if split_pos > 0:
                prefix = info["name"][:split_pos]
                short_name = info["name"][split_pos + 1 :]
                if (
                    len(prefix.encode(encoding, errors)) <= LENGTH_PREFIX
                    and len(short_name.encode(encoding, errors)) <= LENGTH_NAME
                ):
                    info["prefix"] = prefix
                    info["name"] = short_name
                else:
                    raise ValueError("name is too long for ustar format")
            else:
                raise ValueError("name is too long for ustar format")
        return _create_header(info, USTAR_FORMAT, encoding, errors)

    def _create_gnu(self, info, encoding, errors):
        info["magic"] = GNU_MAGIC
        buf = b""
        name_b = info["name"].encode(encoding, errors)
        if len(name_b) > LENGTH_NAME:
            buf += _create_gnu_long_header(name_b, GNUTYPE_LONGNAME)
        link_b = info["linkname"].encode(encoding, errors)
        if len(link_b) > LENGTH_LINK:
            buf += _create_gnu_long_header(link_b, GNUTYPE_LONGLINK)
        return buf + _create_header(info, GNU_FORMAT, encoding, errors)

    def _create_pax(self, info, encoding, errors):
        info["magic"] = POSIX_MAGIC
        pax_headers = dict(self.pax_headers)

        def _needs_pax_str(val, max_len):
            try:
                b = val.encode("ascii")
            except UnicodeEncodeError:
                return True
            return len(b) > max_len

        if _needs_pax_str(info["name"], LENGTH_NAME):
            pax_headers["path"] = info["name"]
            info["name"] = info["name"].encode("ascii", "replace").decode("ascii")[:LENGTH_NAME]
        if _needs_pax_str(info["linkname"], LENGTH_LINK):
            pax_headers["linkpath"] = info["linkname"]
            info["linkname"] = info["linkname"].encode("ascii", "replace").decode("ascii")[:LENGTH_LINK]
        if _needs_pax_str(info["uname"], 32):
            pax_headers["uname"] = info["uname"]
            info["uname"] = info["uname"].encode("ascii", "replace").decode("ascii")[:32]
        if _needs_pax_str(info["gname"], 32):
            pax_headers["gname"] = info["gname"]
            info["gname"] = info["gname"].encode("ascii", "replace").decode("ascii")[:32]

        for field, max_val in (("uid", 0o7777777), ("gid", 0o7777777), ("size", 0o77777777777)):
            val = info[field]
            if val < 0 or val > max_val:
                pax_headers[field] = str(val)
                info[field] = 0

        mtime = info["mtime"]
        if isinstance(mtime, float) and mtime != int(mtime):
            pax_headers["mtime"] = str(mtime)
            info["mtime"] = int(mtime)
        elif mtime < 0 or mtime > 0o77777777777:
            pax_headers["mtime"] = str(mtime)
            info["mtime"] = 0

        buf = b""
        if pax_headers:
            buf += _create_pax_header(pax_headers, XHDTYPE)
        return buf + _create_header(info, USTAR_FORMAT, "ascii", "replace")

    @classmethod
    def frombuf(cls, buf, encoding=ENCODING, errors="surrogateescape"):
        if len(buf) == 0:
            raise EmptyHeaderError("empty header")
        if len(buf) != BLOCKSIZE:
            raise TruncatedHeaderError("truncated header")
        if buf.count(NUL) == BLOCKSIZE:
            raise EOFHeaderError("end of file header")

        chksum = _nti(buf[148:156])
        if chksum not in _calc_chksums(buf):
            raise InvalidHeaderError("bad checksum")

        obj = cls()
        obj.name = _nts(buf[0:100], encoding, errors)
        obj.mode = _nti(buf[100:108])
        obj.uid = _nti(buf[108:116])
        obj.gid = _nti(buf[116:124])
        obj.size = _nti(buf[124:136])
        obj.mtime = _nti(buf[136:148])
        obj.chksum = chksum
        obj.type = buf[156:157]
        obj.linkname = _nts(buf[157:257], encoding, errors)
        obj.uname = _nts(buf[265:297], encoding, errors)
        obj.gname = _nts(buf[297:329], encoding, errors)
        obj.devmajor = _nti(buf[329:337])
        obj.devminor = _nti(buf[337:345])
        prefix = _nts(buf[345:500], encoding, errors)

        if obj.type == AREGTYPE and obj.name.endswith("/"):
            obj.type = DIRTYPE

        if obj.type not in (GNUTYPE_LONGNAME, GNUTYPE_LONGLINK, XHDTYPE, XGLTYPE):
            if prefix:
                obj.name = prefix + "/" + obj.name

        if obj.isdir():
            obj.name = obj.name.rstrip("/")

        if obj.type == AREGTYPE:
            obj.type = REGTYPE

        return obj


class TarFile:
    """The TarFile Class provides an interface to tar archives."""

    format = DEFAULT_FORMAT
    tarinfo = TarInfo

    def __init__(
        self,
        name=None,
        mode="r",
        fileobj=None,
        format=None,
        tarinfo=None,
        dereference=False,
        ignore_zeros=False,
        encoding=None,
        errors="surrogateescape",
        pax_headers=None,
        debug=0,
        errorlevel=1,
        *,
        extraction_filter=None,
    ):
        if mode not in ("r", "a", "w", "x"):
            raise ValueError("mode must be 'r', 'a', 'w' or 'x'")

        self.mode = mode
        self._mode = {"r": "rb", "a": "r+b", "w": "wb", "x": "xb"}[mode]
        self._disk_writeback_path = None
        self._extfileobj = True
        self._close_wrappers = []

        if fileobj is None:
            if name is None:
                raise ValueError("nothing to open")
            path_s = _fspath_str(name)
            if mode == "a":
                existing = b""
                if _os.path.exists(path_s):
                    with _builtins.open(path_s, "rb") as rf:
                        existing = rf.read()
                fileobj = _io.BytesIO(existing)
                self._disk_writeback_path = path_s
                self._extfileobj = False
            elif mode == "x":
                if _os.path.exists(path_s):
                    raise FileExistsError("File exists: %r" % path_s)
                fileobj = _builtins.open(path_s, "wb")
                self._extfileobj = False
            else:
                fileobj = _builtins.open(path_s, self._mode)
                self._extfileobj = False
            self.name = _os.path.abspath(path_s)
        else:
            if name is None and hasattr(fileobj, "name") and isinstance(fileobj.name, (str, bytes)):
                name = fileobj.name
            self.name = _os.path.abspath(_fspath_str(name)) if name else None

        self.fileobj = fileobj
        self.format = DEFAULT_FORMAT if format is None else format
        self.tarinfo = TarInfo if tarinfo is None else tarinfo
        self.dereference = dereference
        self.ignore_zeros = ignore_zeros
        self.encoding = ENCODING if encoding is None else encoding
        self.errors = errors
        self.pax_headers = dict(pax_headers) if pax_headers else {}
        self.debug = debug
        self.errorlevel = errorlevel
        self.extraction_filter = extraction_filter

        self.closed = False
        self.members = []
        self._loaded = False
        self.offset = 0
        self._iter_idx = 0
        self._raw_cache = None

        if self.mode == "r":
            self._load_all_members()
        elif self.mode == "a":
            self._prepare_append()

    def _prepare_append(self):
        # Position right after the last valid member (stripping EOF zero blocks).
        try:
            self.fileobj.seek(0)
            raw = self.fileobj.read()
        except Exception:
            raw = b""
        if not raw:
            self.offset = 0
            return
        # Scan existing members to find exact end offset before trailing zero blocks.
        bio = _io.BytesIO(raw)
        end_offset = 0
        try:
            tmp_members, end_offset = self._scan_stream(bio)
            self.members = tmp_members
        except ReadError:
            raise
        self.fileobj.seek(end_offset)
        if hasattr(self.fileobj, "truncate"):
            self.fileobj.truncate(end_offset)
        self.offset = end_offset

    def _scan_stream(self, fp):
        """Read all members from `fp` (a binary stream), returning `(members, end_offset)`."""
        members = []
        global_pax = {}
        pending_pax = {}
        pending_longname = None
        pending_longlink = None
        offset = 0

        while True:
            buf = fp.read(BLOCKSIZE)
            if not buf:
                if offset == 0 and not members:
                    raise ReadError("empty file")
                break
            if len(buf) < BLOCKSIZE:
                if not members:
                    raise ReadError("truncated header")
                break
            try:
                ti = self.tarinfo.frombuf(buf, self.encoding, self.errors)
            except EOFHeaderError:
                break
            except HeaderError as exc:
                if not members:
                    raise ReadError(str(exc)) from exc
                raise ReadError(str(exc)) from exc

            ti.offset = offset
            offset += BLOCKSIZE
            ti.offset_data = offset

            # Compute data + padding size.
            blocks = (ti.size + BLOCKSIZE - 1) // BLOCKSIZE
            data_bytes = blocks * BLOCKSIZE

            if ti.type == XHDTYPE:
                raw_pax = fp.read(data_bytes)[: ti.size]
                offset += data_bytes
                pending_pax.update(_parse_pax_records(raw_pax))
                continue
            if ti.type == XGLTYPE:
                raw_pax = fp.read(data_bytes)[: ti.size]
                offset += data_bytes
                global_pax.update(_parse_pax_records(raw_pax))
                continue
            if ti.type == GNUTYPE_LONGNAME:
                raw_name = fp.read(data_bytes)[: ti.size]
                offset += data_bytes
                pending_longname = _nts(raw_name, self.encoding, self.errors)
                continue
            if ti.type == GNUTYPE_LONGLINK:
                raw_link = fp.read(data_bytes)[: ti.size]
                offset += data_bytes
                pending_longlink = _nts(raw_link, self.encoding, self.errors)
                continue

            # Apply PAX and GNU extended headers.
            combined_pax = {}
            combined_pax.update(global_pax)
            combined_pax.update(pending_pax)
            pending_pax = {}

            if combined_pax:
                ti.pax_headers = combined_pax
                if "path" in combined_pax:
                    ti.name = combined_pax["path"]
                if "linkpath" in combined_pax:
                    ti.linkname = combined_pax["linkpath"]
                if "uname" in combined_pax:
                    ti.uname = combined_pax["uname"]
                if "gname" in combined_pax:
                    ti.gname = combined_pax["gname"]
                if "uid" in combined_pax:
                    ti.uid = int(combined_pax["uid"])
                if "gid" in combined_pax:
                    ti.gid = int(combined_pax["gid"])
                if "size" in combined_pax:
                    ti.size = int(combined_pax["size"])
                    blocks = (ti.size + BLOCKSIZE - 1) // BLOCKSIZE
                    data_bytes = blocks * BLOCKSIZE
                if "mtime" in combined_pax:
                    try:
                        mt = float(combined_pax["mtime"])
                        ti.mtime = int(mt) if mt == int(mt) else mt
                    except ValueError:
                        pass

            if pending_longname is not None:
                ti.name = pending_longname
                pending_longname = None
            if pending_longlink is not None:
                ti.linkname = pending_longlink
                pending_longlink = None

            if ti.isdir():
                ti.name = ti.name.rstrip("/")

            # Read (and cache) payload bytes for this member.
            payload_padded = fp.read(data_bytes) if data_bytes > 0 else b""
            if len(payload_padded) < ti.size:
                raise ReadError("unexpected end of data")
            ti._cached_data = payload_padded[: ti.size]
            offset += data_bytes

            members.append(ti)

        return members, offset

    def _load_all_members(self):
        if self._loaded:
            return
        self.members, self.offset = self._scan_stream(self.fileobj)
        self._loaded = True

    @classmethod
    def open(cls, name=None, mode="r", fileobj=None, bufsize=RECORDSIZE, **kwargs):
        """Open a tar archive for reading, writing or appending."""
        if not mode:
            raise ValueError("mode must not be empty")

        if mode in ("r", "r:*"):
            return cls._open_auto_read(name, fileobj, **kwargs)

        if ":" in mode:
            filemode, comptype = mode.split(":", 1)
        elif "|" in mode:
            filemode, comptype = mode.split("|", 1)
        else:
            filemode, comptype = mode, ""

        filemode = filemode or "r"
        if comptype == "*":
            if filemode != "r":
                raise ValueError("'*' compression is only allowed in read mode")
            return cls._open_auto_read(name, fileobj, **kwargs)

        if comptype == "":
            return cls.taropen(name, filemode, fileobj, **kwargs)
        if comptype == "gz":
            return cls.gzopen(name, filemode, fileobj, **kwargs)
        raise CompressionError("compression method %r is not supported" % comptype)

    @classmethod
    def _open_auto_read(cls, name, fileobj, **kwargs):
        # Peek magic bytes to decide between gzip and uncompressed tar.
        if fileobj is not None:
            saved_pos = 0
            try:
                saved_pos = fileobj.tell()
            except Exception:
                saved_pos = None
            head = fileobj.read(2)
            if saved_pos is not None:
                fileobj.seek(saved_pos)
            if head == b"\x1f\x8b":
                return cls.gzopen(name, "r", fileobj, **kwargs)
            return cls.taropen(name, "r", fileobj, **kwargs)

        if name is None:
            raise ValueError("nothing to open")
        path_s = _fspath_str(name)
        with _builtins.open(path_s, "rb") as probe:
            head = probe.read(2)
        if head == b"\x1f\x8b":
            return cls.gzopen(path_s, "r", None, **kwargs)
        return cls.taropen(path_s, "r", None, **kwargs)

    @classmethod
    def taropen(cls, name, mode="r", fileobj=None, **kwargs):
        if mode not in ("r", "a", "w", "x"):
            raise ValueError("mode must be 'r', 'a', 'w' or 'x'")
        return cls(name, mode, fileobj, **kwargs)

    @classmethod
    def gzopen(cls, name, mode="r", fileobj=None, compresslevel=9, **kwargs):
        if mode not in ("r", "w", "x"):
            raise ValueError("mode must be 'r', 'w' or 'x'")
        try:
            gzfile = _gzip.GzipFile(name, mode + "b", compresslevel, fileobj)
        except OSError as exc:
            if fileobj is not None and mode == "r":
                raise ReadError("not a gzip file") from exc
            raise
        try:
            t = cls.taropen(name, mode, gzfile, **kwargs)
        except (OSError, EOFError, _gzip.BadGzipFile) as exc:
            gzfile.close()
            raise ReadError("not a gzip file") from exc
        except Exception:
            gzfile.close()
            raise
        t._extfileobj = False
        return t

    def _check(self, mode=None):
        if self.closed:
            raise OSError("%s is closed" % self.__class__.__name__)
        if mode is not None and self.mode not in mode:
            raise OSError("bad operation for mode %r" % self.mode)

    def getmembers(self):
        self._check("ra")
        return list(self.members)

    def getnames(self):
        return [m.name for m in self.getmembers()]

    def getmember(self, name):
        self._check("ra")
        norm = name.rstrip("/") if name != "/" else name
        for m in reversed(self.members):
            if m.name == name or m.name == norm:
                return m
        raise KeyError("filename %r not found" % name)

    def next(self):
        self._check("ra")
        if self._iter_idx < len(self.members):
            m = self.members[self._iter_idx]
            self._iter_idx += 1
            return m
        return None

    def __iter__(self):
        self._check("ra")
        return iter(self.members)

    def gettarinfo(self, name=None, arcname=None, fileobj=None):
        self._check("awx")
        if fileobj is not None:
            name = getattr(fileobj, "name", name)
        path_s = _fspath_str(name)

        if arcname is None:
            arcname = path_s
        else:
            arcname = _fspath_str(arcname)

        drv, arcname = _os.path.splitdrive(arcname)
        arcname = arcname.replace(_os.sep, "/")
        arcname = arcname.lstrip("/")

        statres = _os.stat(path_s) if self.dereference else _os.lstat(path_s)
        stmd = statres.st_mode
        fmt = stmd & _S_IFMT

        tarinfo = self.tarinfo()
        tarinfo.name = arcname
        tarinfo.mode = stmd & 0o7777
        tarinfo.uid = getattr(statres, "st_uid", 0)
        tarinfo.gid = getattr(statres, "st_gid", 0)
        tarinfo.mtime = int(statres.st_mtime)

        if fmt == _S_IFREG:
            tarinfo.type = REGTYPE
            tarinfo.size = statres.st_size
        elif fmt == _S_IFDIR:
            tarinfo.type = DIRTYPE
            tarinfo.size = 0
        elif fmt == _S_IFLNK:
            tarinfo.type = SYMTYPE
            tarinfo.linkname = _os.readlink(path_s)
            tarinfo.size = 0
        elif fmt == _S_IFIFO:
            tarinfo.type = FIFOTYPE
            tarinfo.size = 0
        elif fmt == _S_IFCHR:
            tarinfo.type = CHRTYPE
            tarinfo.size = 0
        elif fmt == _S_IFBLK:
            tarinfo.type = BLKTYPE
            tarinfo.size = 0
        else:
            return None
        return tarinfo

    def addfile(self, tarinfo, fileobj=None):
        self._check("awx")
        ti = _copy.copy(tarinfo)
        buf = ti.tobuf(self.format, self.encoding, self.errors)
        self.fileobj.write(buf)
        ti.offset = self.offset
        self.offset += len(buf)
        ti.offset_data = self.offset

        if fileobj is not None and ti.size > 0:
            remaining = ti.size
            chunks = []
            while remaining > 0:
                chunk = fileobj.read(min(remaining, 16384))
                if not chunk:
                    raise ValueError("end of file reached before tarinfo.size bytes")
                self.fileobj.write(chunk)
                chunks.append(chunk)
                remaining -= len(chunk)
            blocks, rem = divmod(ti.size, BLOCKSIZE)
            if rem > 0:
                self.fileobj.write(NUL * (BLOCKSIZE - rem))
                blocks += 1
            self.offset += blocks * BLOCKSIZE
            ti._cached_data = b"".join(chunks)
        else:
            ti._cached_data = b""

        self.members.append(ti)

    def add(self, name, arcname=None, recursive=True, *, filter=None):
        self._check("awx")
        path_s = _fspath_str(name)
        if arcname is None:
            arcname = path_s
        else:
            arcname = _fspath_str(arcname)

        if self.name is not None and _os.path.abspath(path_s) == self.name:
            return

        tarinfo = self.gettarinfo(path_s, arcname)
        if tarinfo is None:
            return

        if filter is not None:
            tarinfo = filter(tarinfo)
            if tarinfo is None:
                return

        if tarinfo.isreg():
            with _builtins.open(path_s, "rb") as f:
                self.addfile(tarinfo, f)
        elif tarinfo.isdir():
            self.addfile(tarinfo)
            if recursive:
                for entry in sorted(_os.listdir(path_s)):
                    self.add(
                        _os.path.join(path_s, entry),
                        _os.path.join(arcname, entry),
                        recursive=recursive,
                        filter=filter,
                    )
        else:
            self.addfile(tarinfo)

    def extractfile(self, member):
        self._check("ra")
        if isinstance(member, str):
            tarinfo = self.getmember(member)
        else:
            tarinfo = member

        if tarinfo.isreg():
            data = getattr(tarinfo, "_cached_data", b"")
            return _io.BytesIO(data)
        if tarinfo.islnk():
            target = self.getmember(tarinfo.linkname)
            return self.extractfile(target)
        return None

    def _get_filter_function(self, filter):
        if filter is None:
            filter = self.extraction_filter
        if filter is None:
            return fully_trusted_filter
        if isinstance(filter, str):
            try:
                return _NAMED_FILTERS[filter]
            except KeyError:
                raise ValueError(f"filter {filter!r} not found") from None
        return filter

    def _extract_one(self, tarinfo, path, set_attrs, numeric_owner):
        dest = _os.path.join(path, tarinfo.name.replace("/", _os.sep))
        dest = _os.path.normpath(dest)

        if tarinfo.isdir():
            _os.makedirs(dest, exist_ok=True)
            if set_attrs and _os.name != "nt" and tarinfo.mode is not None:
                try:
                    _os.chmod(dest, tarinfo.mode & 0o7777)
                except OSError:
                    pass
            return

        parent = _os.path.dirname(dest)
        if parent and not _os.path.exists(parent):
            _os.makedirs(parent, exist_ok=True)

        if tarinfo.isreg():
            data = getattr(tarinfo, "_cached_data", b"")
            with _builtins.open(dest, "wb") as out:
                out.write(data)
            if set_attrs and _os.name != "nt" and tarinfo.mode is not None:
                try:
                    _os.chmod(dest, tarinfo.mode & 0o7777)
                except OSError:
                    pass
        elif tarinfo.issym():
            if _os.path.lexists(dest):
                _os.unlink(dest)
            _os.symlink(tarinfo.linkname, dest)
        elif tarinfo.islnk():
            target = _os.path.normpath(_os.path.join(path, tarinfo.linkname.replace("/", _os.sep)))
            if _os.path.exists(target):
                _shutil.copyfile(target, dest)
            else:
                # Fallback: extract target member's cached bytes if available.
                try:
                    src_member = self.getmember(tarinfo.linkname)
                    with _builtins.open(dest, "wb") as out:
                        out.write(getattr(src_member, "_cached_data", b""))
                except KeyError as exc:
                    raise ExtractError(f"hardlink target {tarinfo.linkname!r} not found") from exc

    def extract(
        self,
        member,
        path="",
        set_attrs=True,
        *,
        numeric_owner=False,
        filter=None,
    ):
        self._check("ra")
        if isinstance(member, str):
            tarinfo = self.getmember(member)
        else:
            tarinfo = member
        path_s = _fspath_str(path) if path else "."
        filter_fn = self._get_filter_function(filter)
        filtered = filter_fn(tarinfo, path_s)
        if filtered is None:
            return
        self._extract_one(filtered, path_s, set_attrs, numeric_owner)

    def extractall(
        self,
        path=".",
        members=None,
        *,
        numeric_owner=False,
        filter=None,
    ):
        self._check("ra")
        path_s = _fspath_str(path)
        if members is None:
            members = self.getmembers()
        filter_fn = self._get_filter_function(filter)
        dirs = []
        for member in members:
            filtered = filter_fn(member, path_s)
            if filtered is None:
                continue
            if filtered.isdir():
                dirs.append(filtered)
                dest = _os.path.normpath(
                    _os.path.join(path_s, filtered.name.replace("/", _os.sep))
                )
                _os.makedirs(dest, exist_ok=True)
            else:
                self._extract_one(filtered, path_s, True, numeric_owner)
        # Apply directory permissions after extracting files inside them.
        if _os.name != "nt":
            for d in reversed(dirs):
                if d.mode is not None:
                    dest = _os.path.normpath(
                        _os.path.join(path_s, d.name.replace("/", _os.sep))
                    )
                    try:
                        _os.chmod(dest, d.mode & 0o7777)
                    except OSError:
                        pass

    def close(self):
        if self.closed:
            return
        self.closed = True
        try:
            if self.mode in ("a", "w", "x"):
                self.fileobj.write(NUL * (BLOCKSIZE * 2))
                self.offset += BLOCKSIZE * 2
                rem = self.offset % RECORDSIZE
                if rem > 0:
                    self.fileobj.write(NUL * (RECORDSIZE - rem))
                    self.offset += RECORDSIZE - rem
                if self._disk_writeback_path is not None:
                    data = self.fileobj.getvalue()
                    with _builtins.open(self._disk_writeback_path, "wb") as wf:
                        wf.write(data)
        finally:
            self._disk_writeback_path = None
            if not self._extfileobj:
                self.fileobj.close()

    def __enter__(self):
        self._check()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.close()
        return False


open = TarFile.open


def is_tarfile(name):
    """Return True if name points to a tar archive that we are able to handle."""
    try:
        if hasattr(name, "read") and hasattr(name, "seek"):
            pos = name.tell()
            try:
                t = open(fileobj=name, mode="r")
                t.close()
                return True
            finally:
                name.seek(pos)
        else:
            t = open(name, "r")
            t.close()
            return True
    except TarError:
        return False
