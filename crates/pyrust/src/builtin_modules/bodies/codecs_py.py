# Python 3.12 `codecs` standard library module (issue #3203).
#
# Injected by `codecs.rs::inject_python_members` with native helpers:
#   - `_to_bytes(obj)` — bytes-like buffer extraction
#   - `_unicode_name(cp)` — Unicode 15.0.0 character name lookup
#   - `_str_codepoints(s)` — surrogate-safe codepoint list extraction
#   - `_codepoints_to_str(cps)` — surrogate-safe string construction
#   - `_utf8_encode_surrogate(cp)` — 3-byte CESU-8/UTF-8 surrogate encoding
#   - `_native_encode(s, enc, errors)` — fast-path native text encoder

import builtins
import os
import sys

__all__ = [
    "register",
    "lookup",
    "open",
    "EncodedFile",
    "BOM",
    "BOM_BE",
    "BOM_LE",
    "BOM_UTF8",
    "BOM_UTF16",
    "BOM_UTF16_BE",
    "BOM_UTF16_LE",
    "BOM_UTF32",
    "BOM_UTF32_BE",
    "BOM_UTF32_LE",
    "BOM32_LE",
    "BOM32_BE",
    "BOM64_LE",
    "BOM64_BE",
    "CodecInfo",
    "Codec",
    "IncrementalEncoder",
    "IncrementalDecoder",
    "BufferedIncrementalEncoder",
    "BufferedIncrementalDecoder",
    "StreamReader",
    "StreamWriter",
    "StreamReaderWriter",
    "StreamRecoder",
    "getencoder",
    "getdecoder",
    "getincrementalencoder",
    "getincrementaldecoder",
    "getreader",
    "getwriter",
    "encode",
    "decode",
    "iterencode",
    "iterdecode",
    "strict_errors",
    "ignore_errors",
    "replace_errors",
    "xmlcharrefreplace_errors",
    "backslashreplace_errors",
    "namereplace_errors",
    "register_error",
    "lookup_error",
    "unregister",
]

# ── BOM Constants ────────────────────────────────────────────────────────────

BOM_UTF8 = b"\xef\xbb\xbf"
BOM_LE = BOM_UTF16_LE = b"\xff\xfe"
BOM_BE = BOM_UTF16_BE = b"\xfe\xff"
BOM_UTF32_LE = b"\xff\xfe\x00\x00"
BOM_UTF32_BE = b"\x00\x00\xfe\xff"

if sys.byteorder == "little":
    BOM = BOM_UTF16 = BOM_UTF16_LE
    BOM_UTF32 = BOM_UTF32_LE
else:
    BOM = BOM_UTF16 = BOM_UTF16_BE
    BOM_UTF32 = BOM_UTF32_BE

BOM32_LE = BOM_UTF16_LE
BOM32_BE = BOM_UTF16_BE
BOM64_LE = BOM_UTF32_LE
BOM64_BE = BOM_UTF32_BE


# ── CodecInfo ────────────────────────────────────────────────────────────────

class CodecInfo(tuple):
    """Codec details when looking up the codec registry."""

    _is_text_encoding = True

    def __new__(
        cls,
        encode,
        decode,
        streamreader=None,
        streamwriter=None,
        incrementalencoder=None,
        incrementaldecoder=None,
        name=None,
        *,
        _is_text_encoding=None,
    ):
        self = tuple.__new__(cls, (encode, decode, streamreader, streamwriter))
        self.name = name
        self.encode = encode
        self.decode = decode
        self.incrementalencoder = incrementalencoder
        self.incrementaldecoder = incrementaldecoder
        self.streamwriter = streamwriter
        self.streamreader = streamreader
        if _is_text_encoding is not None:
            self._is_text_encoding = _is_text_encoding
        return self

    def __repr__(self):
        return "<%s.%s object for encoding %s at %#x>" % (
            self.__class__.__module__,
            self.__class__.__qualname__,
            self.name,
            id(self),
        )


# ── Base Codec, Incremental, and Stream Classes ──────────────────────────────

class Codec:
    """Defines the interface for stateless encoders/decoders."""

    def encode(self, input, errors="strict"):
        raise NotImplementedError

    def decode(self, input, errors="strict"):
        raise NotImplementedError


class IncrementalEncoder:
    """IncrementalEncoder encodes input in multiple steps."""

    def __init__(self, errors="strict"):
        self.errors = errors
        self.buffer = ""

    def encode(self, input, final=False):
        raise NotImplementedError

    def reset(self):
        pass

    def getstate(self):
        return 0

    def setstate(self, state):
        pass


class BufferedIncrementalEncoder(IncrementalEncoder):
    """Subclass of IncrementalEncoder that buffers unencoded input."""

    def __init__(self, errors="strict"):
        IncrementalEncoder.__init__(self, errors)
        self.buffer = ""

    def _buffer_encode(self, input, errors, final):
        raise NotImplementedError

    def encode(self, input, final=False):
        data = self.buffer + input
        (result, consumed) = self._buffer_encode(data, self.errors, final)
        self.buffer = data[consumed:]
        return result

    def reset(self):
        IncrementalEncoder.reset(self)
        self.buffer = ""

    def getstate(self):
        return self.buffer or 0

    def setstate(self, state):
        self.buffer = state or ""


class IncrementalDecoder:
    """IncrementalDecoder decodes input in multiple steps."""

    def __init__(self, errors="strict"):
        self.errors = errors

    def decode(self, input, final=False):
        raise NotImplementedError

    def reset(self):
        pass

    def getstate(self):
        return (b"", 0)

    def setstate(self, state):
        pass


class BufferedIncrementalDecoder(IncrementalDecoder):
    """Subclass of IncrementalDecoder that buffers undecoded input bytes."""

    def __init__(self, errors="strict"):
        IncrementalDecoder.__init__(self, errors)
        self.buffer = b""

    def _buffer_decode(self, input, errors, final):
        raise NotImplementedError

    def decode(self, input, final=False):
        data = self.buffer + _to_bytes(input)
        (result, consumed) = self._buffer_decode(data, self.errors, final)
        self.buffer = data[consumed:]
        return result

    def reset(self):
        IncrementalDecoder.reset(self)
        self.buffer = b""

    def getstate(self):
        return (self.buffer, 0)

    def setstate(self, state):
        self.buffer = state[0]


class StreamWriter(Codec):
    def __init__(self, stream, errors="strict"):
        self.stream = stream
        self.errors = errors

    def write(self, object):
        data, consumed = self.encode(object, self.errors)
        self.stream.write(data)

    def writelines(self, list):
        self.write("".join(list))

    def reset(self):
        pass

    def seek(self, offset, whence=0):
        self.stream.seek(offset, whence)
        if whence == 0 and offset == 0:
            self.reset()

    def __getattr__(self, name, getattr=getattr):
        return getattr(self.stream, name)

    def __enter__(self):
        return self

    def __exit__(self, type, value, tb):
        self.stream.close()


class StreamReader(Codec):
    charbuffertype = str
    _linebuffertype = None

    def __init__(self, stream, errors="strict"):
        self.stream = stream
        self.errors = errors
        self.bytebuffer = b""
        self._empty_charbuffer = self.charbuffertype()
        self.charbuffer = self._empty_charbuffer
        self.linebuffer = None

    def decode(self, input, errors="strict"):
        raise NotImplementedError

    def read(self, size=-1, chars=-1, firstline=False):
        if self.linebuffer:
            self.charbuffer = self._empty_charbuffer.join(self.linebuffer)
            self.linebuffer = None

        if chars < 0:
            chars = size

        while True:
            if chars >= 0 and len(self.charbuffer) >= chars:
                break
            if size < 0:
                newdata = self.stream.read()
            else:
                newdata = self.stream.read(size)
            data = self.bytebuffer + newdata
            if not data:
                break
            try:
                newchars, decodedbytes = self.decode(data, self.errors)
            except UnicodeDecodeError as exc:
                if firstline:
                    newchars, decodedbytes = self.decode(data[: exc.start], self.errors)
                    lines = newchars.splitlines(keepends=True)
                    if len(lines) <= 1:
                        raise
                else:
                    raise
            self.bytebuffer = data[decodedbytes:]
            self.charbuffer += newchars
            if not newdata:
                break
        if chars < 0:
            result = self.charbuffer
            self.charbuffer = self._empty_charbuffer
        else:
            result = self.charbuffer[:chars]
            self.charbuffer = self.charbuffer[chars:]
        return result

    def readline(self, size=None, keepends=True):
        if self.linebuffer:
            line = self.linebuffer[0]
            del self.linebuffer[0]
            if len(self.linebuffer) == 1:
                self.charbuffer = self.linebuffer[0]
                self.linebuffer = None
            if not keepends:
                line = line.splitlines(keepends=False)[0]
            return line

        readsize = size or 72
        line = self._empty_charbuffer
        while True:
            data = self.read(readsize, firstline=True)
            if data:
                if (isinstance(data, str) and data.endswith("\r")) or (
                    isinstance(data, bytes) and data.endswith(b"\r")
                ):
                    data += self.read(size=1, chars=1)

            line += data
            lines = line.splitlines(keepends=True)
            if lines:
                if len(lines) > 1:
                    line = lines[0]
                    del lines[0]
                    if len(lines) > 1:
                        lines[-1] += self.charbuffer
                        self.linebuffer = lines
                        self.charbuffer = None
                    else:
                        self.charbuffer = lines[0] + self.charbuffer
                    if not keepends:
                        line = line.splitlines(keepends=False)[0]
                    break
                line0withend = lines[0]
                line0withoutend = lines[0].splitlines(keepends=False)[0]
                if line0withend != line0withoutend:
                    self.charbuffer = self._empty_charbuffer.join(lines[1:]) + self.charbuffer
                    if keepends:
                        line = line0withend
                    else:
                        line = line0withoutend
                    break
            if not data or size is not None:
                if line and not keepends:
                    line = line.splitlines(keepends=False)[0]
                break
            if readsize < 8000:
                readsize *= 2
        return line

    def readlines(self, sizehint=None, keepends=True):
        data = self.read()
        return data.splitlines(keepends)

    def reset(self):
        self.bytebuffer = b""
        self.charbuffer = self._empty_charbuffer
        self.linebuffer = None

    def seek(self, offset, whence=0):
        self.stream.seek(offset, whence)
        self.reset()

    def __next__(self):
        line = self.readline()
        if line:
            return line
        raise StopIteration

    def __iter__(self):
        return self

    def __getattr__(self, name, getattr=getattr):
        return getattr(self.stream, name)

    def __enter__(self):
        return self

    def __exit__(self, type, value, tb):
        self.stream.close()


class StreamReaderWriter:
    encoding = "unknown"

    def __init__(self, stream, Reader, Writer, errors="strict"):
        self.stream = stream
        self.reader = Reader(stream, errors)
        self.writer = Writer(stream, errors)
        self.errors = errors

    def read(self, size=-1):
        return self.reader.read(size)

    def readline(self, size=None):
        return self.reader.readline(size)

    def readlines(self, sizehint=None):
        return self.reader.readlines(sizehint)

    def __next__(self):
        return next(self.reader)

    def __iter__(self):
        return self

    def write(self, data):
        return self.writer.write(data)

    def writelines(self, list):
        return self.writer.writelines(list)

    def reset(self):
        self.reader.reset()
        self.writer.reset()

    def seek(self, offset, whence=0):
        self.stream.seek(offset, whence)
        self.reader.reset()
        if whence == 0 and offset == 0:
            self.writer.reset()

    def __getattr__(self, name, getattr=getattr):
        return getattr(self.stream, name)

    def __enter__(self):
        return self

    def __exit__(self, type, value, tb):
        self.stream.close()


class StreamRecoder:
    data_encoding = "unknown"
    file_encoding = "unknown"

    def __init__(self, stream, encode, decode, Reader, Writer, errors="strict"):
        self.stream = stream
        self.encode = encode
        self.decode = decode
        self.reader = Reader(stream, errors)
        self.writer = Writer(stream, errors)
        self.errors = errors

    def read(self, size=-1):
        data = self.reader.read(size)
        data, bytesencoded = self.encode(data, self.errors)
        return data

    def readline(self, size=None):
        if size is None:
            data = self.reader.readline()
        else:
            data = self.reader.readline(size)
        data, bytesencoded = self.encode(data, self.errors)
        return data

    def readlines(self, sizehint=None):
        data = self.reader.read()
        data, bytesencoded = self.encode(data, self.errors)
        return data.splitlines(keepends=True)

    def __next__(self):
        data = next(self.reader)
        data, bytesencoded = self.encode(data, self.errors)
        return data

    def __iter__(self):
        return self

    def write(self, data):
        data, bytesdecoded = self.decode(data, self.errors)
        return self.writer.write(data)

    def writelines(self, list):
        data = b"".join(list)
        data, bytesdecoded = self.decode(data, self.errors)
        return self.writer.write(data)

    def reset(self):
        self.reader.reset()
        self.writer.reset()

    def seek(self, offset, whence=0):
        self.stream.seek(offset, whence)
        self.reader.reset()
        self.writer.reset()

    def __getattr__(self, name, getattr=getattr):
        return getattr(self.stream, name)

    def __enter__(self):
        return self

    def __exit__(self, type, value, tb):
        self.stream.close()


# ── Error Handler Registry & Built-in Handlers ───────────────────────────────

_error_registry = {}


def _check_unicode_exc(exc, allowed, handler_name=None):
    if not isinstance(exc, allowed):
        raise TypeError(
            "don't know how to handle %s in error callback" % type(exc).__name__
        )


def _escape_codepoint(cp):
    if cp < 0x100:
        return "\\x%02x" % cp
    if cp < 0x10000:
        return "\\u%04x" % cp
    return "\\U%08x" % cp


def strict_errors(exception):
    if isinstance(exception, BaseException):
        raise exception
    raise TypeError("codec must pass exception instance")


def ignore_errors(exception):
    _check_unicode_exc(
        exception, (UnicodeEncodeError, UnicodeDecodeError, UnicodeTranslateError)
    )
    return ("", exception.end)


def replace_errors(exception):
    if isinstance(exception, UnicodeEncodeError):
        return ("?" * (exception.end - exception.start), exception.end)
    if isinstance(exception, UnicodeDecodeError):
        return ("\ufffd", exception.end)
    if isinstance(exception, UnicodeTranslateError):
        return ("\ufffd" * (exception.end - exception.start), exception.end)
    raise TypeError(
        "don't know how to handle %s in error callback" % type(exception).__name__
    )


def xmlcharrefreplace_errors(exception):
    _check_unicode_exc(exception, (UnicodeEncodeError,))
    cps = _str_codepoints(exception.object)
    out = []
    for cp in cps[exception.start : exception.end]:
        out.append("&#%d;" % cp)
    return ("".join(out), exception.end)


def backslashreplace_errors(exception):
    if isinstance(exception, UnicodeDecodeError):
        raw = _to_bytes(exception.object)
        out = [("\\x%02x" % b) for b in raw[exception.start : exception.end]]
        return ("".join(out), exception.end)
    if isinstance(exception, (UnicodeEncodeError, UnicodeTranslateError)):
        cps = _str_codepoints(exception.object)
        out = [_escape_codepoint(cp) for cp in cps[exception.start : exception.end]]
        return ("".join(out), exception.end)
    raise TypeError(
        "don't know how to handle %s in error callback" % type(exception).__name__
    )


def namereplace_errors(exception):
    _check_unicode_exc(exception, (UnicodeEncodeError,))
    cps = _str_codepoints(exception.object)
    out = []
    for cp in cps[exception.start : exception.end]:
        uname = _unicode_name(cp)
        if uname is not None:
            out.append("\\N{%s}" % uname)
        else:
            out.append(_escape_codepoint(cp))
    return ("".join(out), exception.end)


def _surrogateescape_errors(exception):
    if isinstance(exception, UnicodeEncodeError):
        cps = _str_codepoints(exception.object)
        out = bytearray()
        for cp in cps[exception.start : exception.end]:
            if 0xDC80 <= cp <= 0xDCFF:
                out.append(cp - 0xDC00)
            else:
                raise exception
        return (bytes(out), exception.end)
    if isinstance(exception, UnicodeDecodeError):
        raw = _to_bytes(exception.object)
        start = exception.start
        end = exception.end
        out = []
        consumed = 0
        limit = min(end, start + 4)
        for i in range(start, limit):
            b = raw[i]
            if b < 0x80:
                break
            out.append(0xDC00 + b)
            consumed += 1
        if consumed == 0:
            raise exception
        return (_codepoints_to_str(out), start + consumed)
    raise TypeError(
        "don't know how to handle %s in error callback" % type(exception).__name__
    )


def _surrogatepass_errors(exception):
    if isinstance(exception, UnicodeEncodeError):
        enc = _normalize_encoding(exception.encoding)
        cps = _str_codepoints(exception.object)
        out = bytearray()
        for cp in cps[exception.start : exception.end]:
            if not (0xD800 <= cp <= 0xDFFF):
                raise exception
            if enc in ("utf_8", "utf8", "u8", "utf", "utf_8_sig"):
                out.extend(_utf8_encode_surrogate(cp))
            elif enc in ("utf_16", "utf16", "utf_16_le", "utf16le"):
                out.append(cp & 0xFF)
                out.append((cp >> 8) & 0xFF)
            elif enc in ("utf_16_be", "utf16be"):
                out.append((cp >> 8) & 0xFF)
                out.append(cp & 0xFF)
            elif enc in ("utf_32", "utf32", "utf_32_le", "utf32le"):
                out.extend(cp.to_bytes(4, "little"))
            elif enc in ("utf_32_be", "utf32be"):
                out.extend(cp.to_bytes(4, "big"))
            else:
                raise exception
        return (bytes(out), exception.end)
    if isinstance(exception, UnicodeDecodeError):
        enc = _normalize_encoding(exception.encoding)
        raw = _to_bytes(exception.object)
        start = exception.start
        if enc in ("utf_8", "utf8", "u8", "utf", "utf_8_sig"):
            if start + 3 <= len(raw) and raw[start] == 0xED:
                b1 = raw[start + 1]
                b2 = raw[start + 2]
                if 0xA0 <= b1 <= 0xBF and 0x80 <= b2 <= 0xBF:
                    cp = ((0xED & 0x0F) << 12) | ((b1 & 0x3F) << 6) | (b2 & 0x3F)
                    return (_codepoints_to_str([cp]), start + 3)
        elif enc in ("utf_16", "utf16", "utf_16_le", "utf16le"):
            if start + 2 <= len(raw):
                u = raw[start] | (raw[start + 1] << 8)
                if 0xD800 <= u <= 0xDFFF:
                    return (_codepoints_to_str([u]), start + 2)
        elif enc in ("utf_16_be", "utf16be"):
            if start + 2 <= len(raw):
                u = (raw[start] << 8) | raw[start + 1]
                if 0xD800 <= u <= 0xDFFF:
                    return (_codepoints_to_str([u]), start + 2)
        elif enc in ("utf_32", "utf32", "utf_32_le", "utf32le"):
            if start + 4 <= len(raw):
                u = int.from_bytes(raw[start : start + 4], "little")
                if 0xD800 <= u <= 0xDFFF:
                    return (_codepoints_to_str([u]), start + 4)
        elif enc in ("utf_32_be", "utf32be"):
            if start + 4 <= len(raw):
                u = int.from_bytes(raw[start : start + 4], "big")
                if 0xD800 <= u <= 0xDFFF:
                    return (_codepoints_to_str([u]), start + 4)
        raise exception
    raise TypeError(
        "don't know how to handle %s in error callback" % type(exception).__name__
    )


_error_registry["strict"] = strict_errors
_error_registry["ignore"] = ignore_errors
_error_registry["replace"] = replace_errors
_error_registry["xmlcharrefreplace"] = xmlcharrefreplace_errors
_error_registry["backslashreplace"] = backslashreplace_errors
_error_registry["namereplace"] = namereplace_errors
_error_registry["surrogateescape"] = _surrogateescape_errors
_error_registry["surrogatepass"] = _surrogatepass_errors


def register_error(name, error_handler):
    if not isinstance(name, str):
        raise TypeError("register_error() argument 1 must be str, not %s" % type(name).__name__)
    if not callable(error_handler):
        raise TypeError("handler must be callable")
    _error_registry[name] = error_handler


def lookup_error(name):
    if not isinstance(name, str):
        raise TypeError("lookup_error() argument must be str, not %s" % type(name).__name__)
    try:
        return _error_registry[name]
    except KeyError:
        raise LookupError("unknown error handler name '%s'" % name)


def _invoke_encode_error(errors, exc, encode_str_fn):
    handler = lookup_error(errors)
    res = handler(exc)
    if not isinstance(res, tuple) or len(res) != 2:
        raise TypeError("encoding error handler must return (str/bytes, int) tuple")
    rep, new_pos = res
    if not isinstance(new_pos, int):
        raise TypeError("encoding error handler must return (str/bytes, int) tuple")
    obj_len = len(exc.object)
    if new_pos < 0:
        new_pos += obj_len
    if new_pos < 0 or new_pos > obj_len:
        raise IndexError("position %d from error handler out of bounds" % new_pos)
    if isinstance(rep, bytes):
        return (rep, new_pos)
    if isinstance(rep, str):
        return (encode_str_fn(rep), new_pos)
    raise TypeError(
        "encoding error handler must return (str/bytes, int) tuple, not %s"
        % type(rep).__name__
    )


def _invoke_decode_error(errors, exc):
    handler = lookup_error(errors)
    res = handler(exc)
    if not isinstance(res, tuple) or len(res) != 2:
        raise TypeError("decoding error handler must return (str, int) tuple")
    rep, new_pos = res
    if not isinstance(rep, str) or not isinstance(new_pos, int):
        raise TypeError("decoding error handler must return (str, int) tuple")
    obj_len = len(exc.object)
    if new_pos < 0:
        new_pos += obj_len
    if new_pos < 0 or new_pos > obj_len:
        raise IndexError("position %d from error handler out of bounds" % new_pos)
    return (rep, new_pos)


# ── Low-Level Codec Functions ────────────────────────────────────────────────

def _require_str(obj, func_name):
    if not isinstance(obj, str):
        raise TypeError(
            "%s() argument 1 must be str, not %s" % (func_name, type(obj).__name__)
        )


def _check_errors_arg(errors):
    if errors is None:
        return "strict"
    if not isinstance(errors, str):
        raise TypeError("errors must be a string, not %s" % type(errors).__name__)
    return errors


# -- ASCII --

def _ascii_encode_strict(s):
    out = bytearray()
    cps = _str_codepoints(s)
    for i, cp in enumerate(cps):
        if cp < 0x80:
            out.append(cp)
        else:
            j = i + 1
            while j < len(cps) and cps[j] >= 0x80:
                j += 1
            raise UnicodeEncodeError("ascii", s, i, j, "ordinal not in range(128)")
    return bytes(out)


def ascii_encode(input, errors="strict"):
    _require_str(input, "ascii_encode")
    errors = _check_errors_arg(errors)
    if input.isascii():
        return (_native_encode(input, "ascii", "strict"), len(input))
    cps = _str_codepoints(input)
    n = len(cps)
    out = bytearray()
    i = 0
    while i < n:
        cp = cps[i]
        if cp < 0x80:
            out.append(cp)
            i += 1
        else:
            j = i + 1
            while j < n and cps[j] >= 0x80:
                j += 1
            exc = UnicodeEncodeError("ascii", input, i, j, "ordinal not in range(128)")
            rep, i = _invoke_encode_error(errors, exc, _ascii_encode_strict)
            out.extend(rep)
    return (bytes(out), n)


def ascii_decode(input, errors="strict"):
    raw = _to_bytes(input)
    errors = _check_errors_arg(errors)
    n = len(raw)
    out = []
    i = 0
    while i < n:
        b = raw[i]
        if b < 0x80:
            out.append(chr(b))
            i += 1
        else:
            exc = UnicodeDecodeError("ascii", raw, i, i + 1, "ordinal not in range(128)")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
    return ("".join(out), n)


# -- Latin-1 / ISO-8859-1 --

def _latin1_encode_strict(s):
    out = bytearray()
    cps = _str_codepoints(s)
    for i, cp in enumerate(cps):
        if cp < 0x100:
            out.append(cp)
        else:
            j = i + 1
            while j < len(cps) and cps[j] >= 0x100:
                j += 1
            raise UnicodeEncodeError("latin-1", s, i, j, "ordinal not in range(256)")
    return bytes(out)


def latin_1_encode(input, errors="strict"):
    _require_str(input, "latin_1_encode")
    errors = _check_errors_arg(errors)
    if input.isascii():
        return (_native_encode(input, "latin-1", "strict"), len(input))
    cps = _str_codepoints(input)
    n = len(cps)
    out = bytearray()
    i = 0
    while i < n:
        cp = cps[i]
        if cp < 0x100:
            out.append(cp)
            i += 1
        else:
            j = i + 1
            while j < n and cps[j] >= 0x100:
                j += 1
            exc = UnicodeEncodeError("latin-1", input, i, j, "ordinal not in range(256)")
            rep, i = _invoke_encode_error(errors, exc, _latin1_encode_strict)
            out.extend(rep)
    return (bytes(out), n)


def latin_1_decode(input, errors="strict"):
    raw = _to_bytes(input)
    _check_errors_arg(errors)
    return ("".join(chr(b) for b in raw), len(raw))


# -- UTF-8 --

def _utf8_encode_strict(s):
    return _native_encode(s, "utf-8", "strict")


def utf_8_encode(input, errors="strict"):
    _require_str(input, "utf_8_encode")
    errors = _check_errors_arg(errors)
    try:
        return (_native_encode(input, "utf-8", "strict"), len(input))
    except UnicodeEncodeError:
        pass
    cps = _str_codepoints(input)
    n = len(cps)
    out = bytearray()
    i = 0
    while i < n:
        cp = cps[i]
        if not (0xD800 <= cp <= 0xDFFF):
            out.extend(_native_encode(chr(cp), "utf-8", "strict"))
            i += 1
        else:
            j = i + 1
            while j < n and 0xD800 <= cps[j] <= 0xDFFF:
                j += 1
            exc = UnicodeEncodeError("utf-8", input, i, j, "surrogates not allowed")
            rep, i = _invoke_encode_error(errors, exc, _utf8_encode_strict)
            out.extend(rep)
    return (bytes(out), n)


def _utf8_decode_impl(raw, errors, final, codec_name="utf-8"):
    n = len(raw)
    if n == 0:
        return ("", 0)
    # Fast path for valid UTF-8
    try:
        return (raw.decode("utf-8"), n)
    except UnicodeDecodeError:
        pass

    out = []
    i = 0
    while i < n:
        b0 = raw[i]
        if b0 < 0x80:
            out.append(chr(b0))
            i += 1
            continue
        if b0 < 0xC2 or b0 > 0xF4:
            exc = UnicodeDecodeError(codec_name, raw, i, i + 1, "invalid start byte")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
            continue
        if b0 < 0xE0:
            # 2-byte sequence
            if i + 1 >= n:
                if not final:
                    break
                exc = UnicodeDecodeError(codec_name, raw, i, n, "unexpected end of data")
                rep, i = _invoke_decode_error(errors, exc)
                out.append(rep)
                continue
            b1 = raw[i + 1]
            if not (0x80 <= b1 <= 0xBF):
                exc = UnicodeDecodeError(codec_name, raw, i, i + 1, "invalid continuation byte")
                rep, i = _invoke_decode_error(errors, exc)
                out.append(rep)
                continue
            cp = ((b0 & 0x1F) << 6) | (b1 & 0x3F)
            out.append(chr(cp))
            i += 2
            continue
        if b0 < 0xF0:
            # 3-byte sequence
            if i + 1 >= n:
                if not final:
                    break
                exc = UnicodeDecodeError(codec_name, raw, i, n, "unexpected end of data")
                rep, i = _invoke_decode_error(errors, exc)
                out.append(rep)
                continue
            b1 = raw[i + 1]
            lo1 = 0xA0 if b0 == 0xE0 else 0x80
            hi1 = 0x9F if b0 == 0xED else 0xBF
            if not (lo1 <= b1 <= hi1):
                exc = UnicodeDecodeError(codec_name, raw, i, i + 1, "invalid continuation byte")
                rep, i = _invoke_decode_error(errors, exc)
                out.append(rep)
                continue
            if i + 2 >= n:
                if not final:
                    break
                exc = UnicodeDecodeError(codec_name, raw, i, n, "unexpected end of data")
                rep, i = _invoke_decode_error(errors, exc)
                out.append(rep)
                continue
            b2 = raw[i + 2]
            if not (0x80 <= b2 <= 0xBF):
                exc = UnicodeDecodeError(codec_name, raw, i, i + 2, "invalid continuation byte")
                rep, i = _invoke_decode_error(errors, exc)
                out.append(rep)
                continue
            cp = ((b0 & 0x0F) << 12) | ((b1 & 0x3F) << 6) | (b2 & 0x3F)
            out.append(chr(cp))
            i += 3
            continue
        # 4-byte sequence (0xF0..=0xF4)
        if i + 1 >= n:
            if not final:
                break
            exc = UnicodeDecodeError(codec_name, raw, i, n, "unexpected end of data")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
            continue
        b1 = raw[i + 1]
        lo1 = 0x90 if b0 == 0xF0 else 0x80
        hi1 = 0x8F if b0 == 0xF4 else 0xBF
        if not (lo1 <= b1 <= hi1):
            exc = UnicodeDecodeError(codec_name, raw, i, i + 1, "invalid continuation byte")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
            continue
        if i + 2 >= n:
            if not final:
                break
            exc = UnicodeDecodeError(codec_name, raw, i, n, "unexpected end of data")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
            continue
        b2 = raw[i + 2]
        if not (0x80 <= b2 <= 0xBF):
            exc = UnicodeDecodeError(codec_name, raw, i, i + 2, "invalid continuation byte")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
            continue
        if i + 3 >= n:
            if not final:
                break
            exc = UnicodeDecodeError(codec_name, raw, i, n, "unexpected end of data")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
            continue
        b3 = raw[i + 3]
        if not (0x80 <= b3 <= 0xBF):
            exc = UnicodeDecodeError(codec_name, raw, i, i + 3, "invalid continuation byte")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
            continue
        cp = ((b0 & 0x07) << 18) | ((b1 & 0x3F) << 12) | ((b2 & 0x3F) << 6) | (b3 & 0x3F)
        out.append(chr(cp))
        i += 4
    return ("".join(out), i)


def utf_8_decode(input, errors="strict", final=False):
    raw = _to_bytes(input)
    errors = _check_errors_arg(errors)
    return _utf8_decode_impl(raw, errors, bool(final), "utf-8")


# -- UTF-16 --

def _encode_utf16_cp(cp, big_endian, out):
    if cp < 0x10000:
        if big_endian:
            out.append((cp >> 8) & 0xFF)
            out.append(cp & 0xFF)
        else:
            out.append(cp & 0xFF)
            out.append((cp >> 8) & 0xFF)
    else:
        v = cp - 0x10000
        hi = 0xD800 + (v >> 10)
        lo = 0xDC00 + (v & 0x3FF)
        if big_endian:
            out.append((hi >> 8) & 0xFF)
            out.append(hi & 0xFF)
            out.append((lo >> 8) & 0xFF)
            out.append(lo & 0xFF)
        else:
            out.append(hi & 0xFF)
            out.append((hi >> 8) & 0xFF)
            out.append(lo & 0xFF)
            out.append((lo >> 8) & 0xFF)


def _utf16_encode_impl(input, errors, byteorder, codec_name):
    big_endian = byteorder > 0 or (byteorder == 0 and sys.byteorder == "big")
    out = bytearray()
    if byteorder == 0:
        out.extend(BOM_UTF16_BE if big_endian else BOM_UTF16_LE)

    def _strict_sub(s):
        sub = bytearray()
        for cp in _str_codepoints(s):
            if 0xD800 <= cp <= 0xDFFF:
                raise UnicodeEncodeError(codec_name, s, 0, 1, "surrogates not allowed")
            _encode_utf16_cp(cp, big_endian, sub)
        return bytes(sub)

    cps = _str_codepoints(input)
    n = len(cps)
    i = 0
    while i < n:
        cp = cps[i]
        if not (0xD800 <= cp <= 0xDFFF):
            _encode_utf16_cp(cp, big_endian, out)
            i += 1
        else:
            j = i + 1
            while j < n and 0xD800 <= cps[j] <= 0xDFFF:
                j += 1
            exc = UnicodeEncodeError(codec_name, input, i, j, "surrogates not allowed")
            rep, i = _invoke_encode_error(errors, exc, _strict_sub)
            out.extend(rep)
    return (bytes(out), n)


def utf_16_encode(input, errors="strict", byteorder=0):
    _require_str(input, "utf_16_encode")
    errors = _check_errors_arg(errors)
    return _utf16_encode_impl(input, errors, byteorder, "utf-16")


def utf_16_le_encode(input, errors="strict"):
    _require_str(input, "utf_16_le_encode")
    errors = _check_errors_arg(errors)
    return _utf16_encode_impl(input, errors, -1, "utf-16-le")


def utf_16_be_encode(input, errors="strict"):
    _require_str(input, "utf_16_be_encode")
    errors = _check_errors_arg(errors)
    return _utf16_encode_impl(input, errors, 1, "utf-16-be")


def utf_16_ex_decode(input, errors="strict", byteorder=0, final=False):
    raw = _to_bytes(input)
    errors = _check_errors_arg(errors)
    final = bool(final)
    n = len(raw)
    i = 0
    bo = byteorder
    if bo == 0:
        if n < 2:
            if not final:
                return ("", 0, 0)
            if n == 0:
                return ("", 0, 0)
            bo = -1 if sys.byteorder == "little" else 1
        else:
            if raw[0] == 0xFF and raw[1] == 0xFE:
                bo = -1
                i = 2
            elif raw[0] == 0xFE and raw[1] == 0xFF:
                bo = 1
                i = 2
            else:
                bo = -1 if sys.byteorder == "little" else 1
    big_endian = bo > 0
    codec_name = "utf-16-be" if big_endian else "utf-16-le"
    out = []
    while i < n:
        if i + 2 > n:
            if not final:
                break
            exc = UnicodeDecodeError(codec_name, raw, i, n, "truncated data")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
            continue
        if big_endian:
            u1 = (raw[i] << 8) | raw[i + 1]
        else:
            u1 = raw[i] | (raw[i + 1] << 8)
        if 0xD800 <= u1 <= 0xDBFF:
            if i + 4 > n:
                if not final:
                    break
                exc = UnicodeDecodeError(codec_name, raw, i, n, "unexpected end of data")
                rep, i = _invoke_decode_error(errors, exc)
                out.append(rep)
                continue
            if big_endian:
                u2 = (raw[i + 2] << 8) | raw[i + 3]
            else:
                u2 = raw[i + 2] | (raw[i + 3] << 8)
            if 0xDC00 <= u2 <= 0xDFFF:
                cp = 0x10000 + ((u1 - 0xD800) << 10) + (u2 - 0xDC00)
                out.append(chr(cp))
                i += 4
            else:
                exc = UnicodeDecodeError(codec_name, raw, i, i + 2, "illegal encoding")
                rep, i = _invoke_decode_error(errors, exc)
                out.append(rep)
        elif 0xDC00 <= u1 <= 0xDFFF:
            exc = UnicodeDecodeError(codec_name, raw, i, i + 2, "illegal encoding")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
        else:
            out.append(chr(u1))
            i += 2
    return ("".join(out), i, bo)


def utf_16_decode(input, errors="strict", final=False):
    text, consumed, _ = utf_16_ex_decode(input, errors, 0, final)
    return (text, consumed)


def utf_16_le_decode(input, errors="strict", final=False):
    text, consumed, _ = utf_16_ex_decode(input, errors, -1, final)
    return (text, consumed)


def utf_16_be_decode(input, errors="strict", final=False):
    text, consumed, _ = utf_16_ex_decode(input, errors, 1, final)
    return (text, consumed)


# -- UTF-32 --

def _utf32_encode_impl(input, errors, byteorder, codec_name):
    big_endian = byteorder > 0 or (byteorder == 0 and sys.byteorder == "big")
    endian = "big" if big_endian else "little"
    out = bytearray()
    if byteorder == 0:
        out.extend(BOM_UTF32_BE if big_endian else BOM_UTF32_LE)

    def _strict_sub(s):
        sub = bytearray()
        for cp in _str_codepoints(s):
            if 0xD800 <= cp <= 0xDFFF:
                raise UnicodeEncodeError(codec_name, s, 0, 1, "surrogates not allowed")
            sub.extend(cp.to_bytes(4, endian))
        return bytes(sub)

    cps = _str_codepoints(input)
    n = len(cps)
    i = 0
    while i < n:
        cp = cps[i]
        if not (0xD800 <= cp <= 0xDFFF):
            out.extend(cp.to_bytes(4, endian))
            i += 1
        else:
            j = i + 1
            while j < n and 0xD800 <= cps[j] <= 0xDFFF:
                j += 1
            exc = UnicodeEncodeError(codec_name, input, i, j, "surrogates not allowed")
            rep, i = _invoke_encode_error(errors, exc, _strict_sub)
            out.extend(rep)
    return (bytes(out), n)


def utf_32_encode(input, errors="strict", byteorder=0):
    _require_str(input, "utf_32_encode")
    errors = _check_errors_arg(errors)
    return _utf32_encode_impl(input, errors, byteorder, "utf-32")


def utf_32_le_encode(input, errors="strict"):
    _require_str(input, "utf_32_le_encode")
    errors = _check_errors_arg(errors)
    return _utf32_encode_impl(input, errors, -1, "utf-32-le")


def utf_32_be_encode(input, errors="strict"):
    _require_str(input, "utf_32_be_encode")
    errors = _check_errors_arg(errors)
    return _utf32_encode_impl(input, errors, 1, "utf-32-be")


def utf_32_ex_decode(input, errors="strict", byteorder=0, final=False):
    raw = _to_bytes(input)
    errors = _check_errors_arg(errors)
    final = bool(final)
    n = len(raw)
    i = 0
    bo = byteorder
    if bo == 0:
        if n < 4:
            if not final:
                return ("", 0, 0)
            if n == 0:
                return ("", 0, 0)
            bo = -1 if sys.byteorder == "little" else 1
        else:
            if raw[0:4] == BOM_UTF32_LE:
                bo = -1
                i = 4
            elif raw[0:4] == BOM_UTF32_BE:
                bo = 1
                i = 4
            else:
                bo = -1 if sys.byteorder == "little" else 1
    big_endian = bo > 0
    endian = "big" if big_endian else "little"
    codec_name = "utf-32-be" if big_endian else "utf-32-le"
    out = []
    while i < n:
        if i + 4 > n:
            if not final:
                break
            exc = UnicodeDecodeError(codec_name, raw, i, n, "truncated data")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
            continue
        cp = int.from_bytes(raw[i : i + 4], endian)
        if cp > 0x10FFFF:
            exc = UnicodeDecodeError(codec_name, raw, i, i + 4, "code point not in range(0x110000)")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
        elif 0xD800 <= cp <= 0xDFFF:
            exc = UnicodeDecodeError(codec_name, raw, i, i + 4, "code point in surrogate code point range(0xd800, 0xe000)")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
        else:
            out.append(chr(cp))
            i += 4
    return ("".join(out), i, bo)


def utf_32_decode(input, errors="strict", final=False):
    text, consumed, _ = utf_32_ex_decode(input, errors, 0, final)
    return (text, consumed)


def utf_32_le_decode(input, errors="strict", final=False):
    text, consumed, _ = utf_32_ex_decode(input, errors, -1, final)
    return (text, consumed)


def utf_32_be_decode(input, errors="strict", final=False):
    text, consumed, _ = utf_32_ex_decode(input, errors, 1, final)
    return (text, consumed)


# -- Escape & UTF-7 low-level codecs --

def unicode_escape_encode(input, errors="strict"):
    _require_str(input, "unicode_escape_encode")
    _check_errors_arg(errors)
    return (_native_encode(input, "unicode-escape", "strict"), len(input))


def unicode_escape_decode(input, errors="strict"):
    raw = _to_bytes(input)
    errors = _check_errors_arg(errors)
    if errors in ("strict", "ignore", "replace"):
        return (raw.decode("unicode-escape", errors), len(raw))
    try:
        return (raw.decode("unicode-escape", "strict"), len(raw))
    except UnicodeDecodeError as exc:
        out = []
        pos = 0
        while pos < len(raw):
            try:
                out.append(raw[pos:].decode("unicode-escape", "strict"))
                break
            except UnicodeDecodeError as sub_exc:
                abs_start = pos + sub_exc.start
                abs_end = pos + sub_exc.end
                if sub_exc.start > 0:
                    out.append(raw[pos:abs_start].decode("unicode-escape", "strict"))
                full_exc = UnicodeDecodeError(
                    "unicodeescape", raw, abs_start, abs_end, sub_exc.reason
                )
                rep, pos = _invoke_decode_error(errors, full_exc)
                out.append(rep)
        return ("".join(out), len(raw))


def raw_unicode_escape_encode(input, errors="strict"):
    _require_str(input, "raw_unicode_escape_encode")
    _check_errors_arg(errors)
    return (_native_encode(input, "raw-unicode-escape", "strict"), len(input))


def raw_unicode_escape_decode(input, errors="strict"):
    raw = _to_bytes(input)
    errors = _check_errors_arg(errors)
    if errors in ("strict", "ignore", "replace"):
        return (raw.decode("raw-unicode-escape", errors), len(raw))
    try:
        return (raw.decode("raw-unicode-escape", "strict"), len(raw))
    except UnicodeDecodeError:
        out = []
        pos = 0
        while pos < len(raw):
            try:
                out.append(raw[pos:].decode("raw-unicode-escape", "strict"))
                break
            except UnicodeDecodeError as sub_exc:
                abs_start = pos + sub_exc.start
                abs_end = pos + sub_exc.end
                if sub_exc.start > 0:
                    out.append(raw[pos:abs_start].decode("raw-unicode-escape", "strict"))
                full_exc = UnicodeDecodeError(
                    "rawunicodeescape", raw, abs_start, abs_end, sub_exc.reason
                )
                rep, pos = _invoke_decode_error(errors, full_exc)
                out.append(rep)
        return ("".join(out), len(raw))


def utf_7_encode(input, errors="strict"):
    _require_str(input, "utf_7_encode")
    _check_errors_arg(errors)
    return (_native_encode(input, "utf-7", "strict"), len(input))


def utf_7_decode(input, errors="strict", final=False):
    raw = _to_bytes(input)
    errors = _check_errors_arg(errors)
    return (raw.decode("utf-7", errors), len(raw))


def escape_encode(input, errors="strict"):
    raw = _to_bytes(input)
    _check_errors_arg(errors)
    out = bytearray()
    for b in raw:
        if b == 0x5C:
            out.extend(b"\\\\")
        elif b == 0x27:
            out.extend(b"\\'")
        elif b == 0x09:
            out.extend(b"\\t")
        elif b == 0x0A:
            out.extend(b"\\n")
        elif b == 0x0D:
            out.extend(b"\\r")
        elif 0x20 <= b <= 0x7E:
            out.append(b)
        else:
            out.extend(("\\x%02x" % b).encode("ascii"))
    return (bytes(out), len(raw))


def escape_decode(input, errors="strict"):
    raw = _to_bytes(input)
    _check_errors_arg(errors)
    out = bytearray()
    i = 0
    n = len(raw)
    while i < n:
        b = raw[i]
        if b != 0x5C:
            out.append(b)
            i += 1
            continue
        i += 1
        if i >= n:
            raise ValueError("Trailing \\ in string")
        c = raw[i]
        i += 1
        if c == 0x0A:
            pass
        elif c == 0x5C:
            out.append(0x5C)
        elif c == 0x27:
            out.append(0x27)
        elif c == 0x22:
            out.append(0x22)
        elif c == ord("a"):
            out.append(0x07)
        elif c == ord("b"):
            out.append(0x08)
        elif c == ord("f"):
            out.append(0x0C)
        elif c == ord("n"):
            out.append(0x0A)
        elif c == ord("r"):
            out.append(0x0D)
        elif c == ord("t"):
            out.append(0x09)
        elif c == ord("v"):
            out.append(0x0B)
        elif ord("0") <= c <= ord("7"):
            val = c - ord("0")
            for _ in range(2):
                if i < n and ord("0") <= raw[i] <= ord("7"):
                    val = (val << 3) + (raw[i] - ord("0"))
                    i += 1
                else:
                    break
            out.append(val & 0xFF)
        elif c == ord("x"):
            if i + 2 > n:
                raise ValueError("invalid \\x escape at position %d" % (i - 2))
            try:
                val = int(raw[i : i + 2].decode("ascii"), 16)
            except ValueError:
                raise ValueError("invalid \\x escape at position %d" % (i - 2))
            out.append(val)
            i += 2
        else:
            out.append(0x5C)
            out.append(c)
    return (bytes(out), n)


def readbuffer_encode(input, errors="strict"):
    if isinstance(input, str):
        b = _native_encode(input, "utf-8", "strict")
    else:
        b = _to_bytes(input)
    return (b, len(b))


# -- Charmap helpers --

def make_identity_dict(rng):
    return {i: i for i in rng}


def make_encoding_map(decoding_map):
    m = {}
    for k, v in decoding_map.items():
        if v not in m:
            m[v] = k
        else:
            m[v] = None
    return m


def charmap_build(decoding_table):
    if not isinstance(decoding_table, str):
        raise TypeError("charmap_build() argument must be str, not %s" % type(decoding_table).__name__)
    table = {}
    for i, ch in enumerate(decoding_table):
        cp = ord(ch)
        if cp != 0xFFFE:
            table[cp] = i
    return table


def charmap_encode(input, errors="strict", mapping=None):
    _require_str(input, "charmap_encode")
    errors = _check_errors_arg(errors)
    if mapping is None:
        return latin_1_encode(input, errors)

    def _encode_one(cp):
        if cp in mapping:
            val = mapping[cp]
            if val is None:
                return None
            if isinstance(val, int):
                if 0 <= val <= 255:
                    return bytes([val])
                raise TypeError("character mapping must be in range(256)")
            if isinstance(val, bytes):
                return val
            raise TypeError("character mapping must return integer, bytes or None")
        return None

    def _strict_sub(s):
        sub = bytearray()
        for cp in _str_codepoints(s):
            enc = _encode_one(cp)
            if enc is None:
                raise UnicodeEncodeError("charmap", s, 0, 1, "character maps to <undefined>")
            sub.extend(enc)
        return bytes(sub)

    cps = _str_codepoints(input)
    n = len(cps)
    out = bytearray()
    i = 0
    while i < n:
        enc = _encode_one(cps[i])
        if enc is not None:
            out.extend(enc)
            i += 1
        else:
            j = i + 1
            while j < n and _encode_one(cps[j]) is None:
                j += 1
            exc = UnicodeEncodeError("charmap", input, i, j, "character maps to <undefined>")
            rep, i = _invoke_encode_error(errors, exc, _strict_sub)
            out.extend(rep)
    return (bytes(out), n)


def charmap_decode(input, errors="strict", mapping=None):
    raw = _to_bytes(input)
    errors = _check_errors_arg(errors)
    if mapping is None:
        return latin_1_decode(raw, errors)
    is_str_map = isinstance(mapping, str)
    n = len(raw)
    out = []
    i = 0
    while i < n:
        b = raw[i]
        val = None
        if is_str_map:
            if b < len(mapping):
                ch = mapping[b]
                if ch != "\ufffe":
                    val = ch
        else:
            if b in mapping:
                v = mapping[b]
                if v is None:
                    val = None
                elif isinstance(v, int):
                    if v == 0xFFFE:
                        val = None
                    elif 0 <= v <= 0x10FFFF:
                        val = chr(v)
                    else:
                        raise TypeError("character mapping must be in range(0x110000)")
                elif isinstance(v, str):
                    val = v
                else:
                    raise TypeError("character mapping must return integer, None or str")
        if val is not None:
            out.append(val)
            i += 1
        else:
            exc = UnicodeDecodeError("charmap", raw, i, i + 1, "character maps to <undefined>")
            rep, i = _invoke_decode_error(errors, exc)
            out.append(rep)
    return ("".join(out), n)


# ── Built-in Encodings ───────────────────────────────────────────────────────

# -- UTF-8 --
class _Utf8IncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return utf_8_encode(input, self.errors)[0]


class _Utf8IncDecoder(BufferedIncrementalDecoder):
    _buffer_decode = staticmethod(utf_8_decode)


class _Utf8StreamWriter(StreamWriter):
    encode = staticmethod(utf_8_encode)


class _Utf8StreamReader(StreamReader):
    decode = staticmethod(utf_8_decode)


# -- UTF-8-SIG --
def _utf_8_sig_encode(input, errors="strict"):
    data, consumed = utf_8_encode(input, errors)
    return (BOM_UTF8 + data, consumed)


def _utf_8_sig_decode(input, errors="strict"):
    raw = _to_bytes(input)
    prefix = 0
    if raw[:3] == BOM_UTF8:
        raw = raw[3:]
        prefix = 3
    output, consumed = utf_8_decode(raw, errors, True)
    return (output, consumed + prefix)


class _Utf8SigIncEncoder(IncrementalEncoder):
    def __init__(self, errors="strict"):
        IncrementalEncoder.__init__(self, errors)
        self.first = 1

    def encode(self, input, final=False):
        if self.first:
            self.first = 0
            return BOM_UTF8 + utf_8_encode(input, self.errors)[0]
        return utf_8_encode(input, self.errors)[0]

    def reset(self):
        IncrementalEncoder.reset(self)
        self.first = 1

    def getstate(self):
        return self.first

    def setstate(self, state):
        self.first = state


class _Utf8SigIncDecoder(BufferedIncrementalDecoder):
    def __init__(self, errors="strict"):
        BufferedIncrementalDecoder.__init__(self, errors)
        self.first = 1

    def _buffer_decode(self, input, errors, final):
        if self.first:
            if len(input) < 3:
                if BOM_UTF8.startswith(input):
                    if not final:
                        return ("", 0)
                else:
                    self.first = 0
            else:
                self.first = 0
                if input[:3] == BOM_UTF8:
                    output, consumed = utf_8_decode(input[3:], errors, final)
                    return (output, consumed + 3)
        return utf_8_decode(input, errors, final)

    def reset(self):
        BufferedIncrementalDecoder.reset(self)
        self.first = 1

    def getstate(self):
        state = BufferedIncrementalDecoder.getstate(self)
        return (state[0], self.first)

    def setstate(self, state):
        BufferedIncrementalDecoder.setstate(self, state)
        self.first = state[1]


class _Utf8SigStreamWriter(StreamWriter):
    def reset(self):
        StreamWriter.reset(self)
        try:
            del self.encode
        except AttributeError:
            pass

    def encode(self, input, errors="strict"):
        self.encode = utf_8_encode
        return _utf_8_sig_encode(input, errors)


class _Utf8SigStreamReader(StreamReader):
    def reset(self):
        StreamReader.reset(self)
        try:
            del self.decode
        except AttributeError:
            pass

    def decode(self, input, errors="strict"):
        if len(input) < 3:
            if BOM_UTF8.startswith(input):
                return ("", 0)
        elif input[:3] == BOM_UTF8:
            self.decode = utf_8_decode
            output, consumed = utf_8_decode(input[3:], errors)
            return (output, consumed + 3)
        self.decode = utf_8_decode
        return utf_8_decode(input, errors)


# -- UTF-16 / LE / BE --
class _Utf16IncEncoder(IncrementalEncoder):
    def __init__(self, errors="strict"):
        IncrementalEncoder.__init__(self, errors)
        self.encoder = None

    def encode(self, input, final=False):
        if self.encoder is None:
            result = utf_16_encode(input, self.errors)[0]
            if sys.byteorder == "little":
                self.encoder = utf_16_le_encode
            else:
                self.encoder = utf_16_be_encode
            return result
        return self.encoder(input, self.errors)[0]

    def reset(self):
        IncrementalEncoder.reset(self)
        self.encoder = None

    def getstate(self):
        return 2 if self.encoder is None else 0

    def setstate(self, state):
        if state:
            self.encoder = None
        else:
            if sys.byteorder == "little":
                self.encoder = utf_16_le_encode
            else:
                self.encoder = utf_16_be_encode


class _Utf16IncDecoder(BufferedIncrementalDecoder):
    def __init__(self, errors="strict"):
        BufferedIncrementalDecoder.__init__(self, errors)
        self.decoder = None

    def _buffer_decode(self, input, errors, final):
        if self.decoder is None:
            output, consumed, byteorder = utf_16_ex_decode(input, errors, 0, final)
            if byteorder == -1:
                self.decoder = utf_16_le_decode
            elif byteorder == 1:
                self.decoder = utf_16_be_decode
            elif consumed >= 2:
                raise UnicodeError("UTF-16 stream does not start with BOM")
            return (output, consumed)
        return self.decoder(input, self.errors, final)

    def reset(self):
        BufferedIncrementalDecoder.reset(self)
        self.decoder = None

    def getstate(self):
        state = BufferedIncrementalDecoder.getstate(self)[0]
        if self.decoder is None:
            return (state, 2)
        addstate = int((sys.byteorder == "big") != (self.decoder is utf_16_be_decode))
        return (state, addstate)

    def setstate(self, state):
        BufferedIncrementalDecoder.setstate(self, state)
        s = state[1]
        if s == 0:
            self.decoder = utf_16_be_decode if sys.byteorder == "big" else utf_16_le_decode
        elif s == 1:
            self.decoder = utf_16_le_decode if sys.byteorder == "big" else utf_16_be_decode
        else:
            self.decoder = None


class _Utf16StreamWriter(StreamWriter):
    def __init__(self, stream, errors="strict"):
        StreamWriter.__init__(self, stream, errors)
        self.encoder = None

    def reset(self):
        StreamWriter.reset(self)
        self.encoder = None

    def encode(self, input, errors="strict"):
        if self.encoder is None:
            result = utf_16_encode(input, errors)
            if sys.byteorder == "little":
                self.encoder = utf_16_le_encode
            else:
                self.encoder = utf_16_be_encode
            return result
        return self.encoder(input, errors)


class _Utf16StreamReader(StreamReader):
    def reset(self):
        StreamReader.reset(self)
        try:
            del self.decode
        except AttributeError:
            pass

    def decode(self, input, errors="strict"):
        output, consumed, byteorder = utf_16_ex_decode(input, errors, 0, False)
        if byteorder == -1:
            self.decode = utf_16_le_decode
        elif byteorder == 1:
            self.decode = utf_16_be_decode
        elif consumed >= 2:
            raise UnicodeError("UTF-16 stream does not start with BOM")
        return (output, consumed)


class _Utf16LeIncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return utf_16_le_encode(input, self.errors)[0]


class _Utf16LeIncDecoder(BufferedIncrementalDecoder):
    _buffer_decode = staticmethod(utf_16_le_decode)


class _Utf16LeStreamWriter(StreamWriter):
    encode = staticmethod(utf_16_le_encode)


class _Utf16LeStreamReader(StreamReader):
    decode = staticmethod(utf_16_le_decode)


class _Utf16BeIncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return utf_16_be_encode(input, self.errors)[0]


class _Utf16BeIncDecoder(BufferedIncrementalDecoder):
    _buffer_decode = staticmethod(utf_16_be_decode)


class _Utf16BeStreamWriter(StreamWriter):
    encode = staticmethod(utf_16_be_encode)


class _Utf16BeStreamReader(StreamReader):
    decode = staticmethod(utf_16_be_decode)


# -- UTF-32 / LE / BE --
class _Utf32IncEncoder(IncrementalEncoder):
    def __init__(self, errors="strict"):
        IncrementalEncoder.__init__(self, errors)
        self.encoder = None

    def encode(self, input, final=False):
        if self.encoder is None:
            result = utf_32_encode(input, self.errors)[0]
            if sys.byteorder == "little":
                self.encoder = utf_32_le_encode
            else:
                self.encoder = utf_32_be_encode
            return result
        return self.encoder(input, self.errors)[0]

    def reset(self):
        IncrementalEncoder.reset(self)
        self.encoder = None

    def getstate(self):
        return 2 if self.encoder is None else 0

    def setstate(self, state):
        if state:
            self.encoder = None
        else:
            if sys.byteorder == "little":
                self.encoder = utf_32_le_encode
            else:
                self.encoder = utf_32_be_encode


class _Utf32IncDecoder(BufferedIncrementalDecoder):
    def __init__(self, errors="strict"):
        BufferedIncrementalDecoder.__init__(self, errors)
        self.decoder = None

    def _buffer_decode(self, input, errors, final):
        if self.decoder is None:
            output, consumed, byteorder = utf_32_ex_decode(input, errors, 0, final)
            if byteorder == -1:
                self.decoder = utf_32_le_decode
            elif byteorder == 1:
                self.decoder = utf_32_be_decode
            elif consumed >= 4:
                raise UnicodeError("UTF-32 stream does not start with BOM")
            return (output, consumed)
        return self.decoder(input, self.errors, final)

    def reset(self):
        BufferedIncrementalDecoder.reset(self)
        self.decoder = None

    def getstate(self):
        state = BufferedIncrementalDecoder.getstate(self)[0]
        if self.decoder is None:
            return (state, 2)
        addstate = int((sys.byteorder == "big") != (self.decoder is utf_32_be_decode))
        return (state, addstate)

    def setstate(self, state):
        BufferedIncrementalDecoder.setstate(self, state)
        s = state[1]
        if s == 0:
            self.decoder = utf_32_be_decode if sys.byteorder == "big" else utf_32_le_decode
        elif s == 1:
            self.decoder = utf_32_le_decode if sys.byteorder == "big" else utf_32_be_decode
        else:
            self.decoder = None


class _Utf32StreamWriter(StreamWriter):
    def __init__(self, stream, errors="strict"):
        StreamWriter.__init__(self, stream, errors)
        self.encoder = None

    def reset(self):
        StreamWriter.reset(self)
        self.encoder = None

    def encode(self, input, errors="strict"):
        if self.encoder is None:
            result = utf_32_encode(input, errors)
            if sys.byteorder == "little":
                self.encoder = utf_32_le_encode
            else:
                self.encoder = utf_32_be_encode
            return result
        return self.encoder(input, errors)


class _Utf32StreamReader(StreamReader):
    def reset(self):
        StreamReader.reset(self)
        try:
            del self.decode
        except AttributeError:
            pass

    def decode(self, input, errors="strict"):
        output, consumed, byteorder = utf_32_ex_decode(input, errors, 0, False)
        if byteorder == -1:
            self.decode = utf_32_le_decode
        elif byteorder == 1:
            self.decode = utf_32_be_decode
        elif consumed >= 4:
            raise UnicodeError("UTF-32 stream does not start with BOM")
        return (output, consumed)


class _Utf32LeIncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return utf_32_le_encode(input, self.errors)[0]


class _Utf32LeIncDecoder(BufferedIncrementalDecoder):
    _buffer_decode = staticmethod(utf_32_le_decode)


class _Utf32LeStreamWriter(StreamWriter):
    encode = staticmethod(utf_32_le_encode)


class _Utf32LeStreamReader(StreamReader):
    decode = staticmethod(utf_32_le_decode)


class _Utf32BeIncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return utf_32_be_encode(input, self.errors)[0]


class _Utf32BeIncDecoder(BufferedIncrementalDecoder):
    _buffer_decode = staticmethod(utf_32_be_decode)


class _Utf32BeStreamWriter(StreamWriter):
    encode = staticmethod(utf_32_be_encode)


class _Utf32BeStreamReader(StreamReader):
    decode = staticmethod(utf_32_be_decode)


# -- ASCII --
class _AsciiIncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return ascii_encode(input, self.errors)[0]


class _AsciiIncDecoder(IncrementalDecoder):
    def decode(self, input, final=False):
        return ascii_decode(input, self.errors)[0]


class _AsciiStreamWriter(StreamWriter):
    encode = staticmethod(ascii_encode)


class _AsciiStreamReader(StreamReader):
    decode = staticmethod(ascii_decode)


# -- Latin-1 / ISO-8859-1 --
class _Latin1IncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return latin_1_encode(input, self.errors)[0]


class _Latin1IncDecoder(IncrementalDecoder):
    def decode(self, input, final=False):
        return latin_1_decode(input, self.errors)[0]


class _Latin1StreamWriter(StreamWriter):
    encode = staticmethod(latin_1_encode)


class _Latin1StreamReader(StreamReader):
    decode = staticmethod(latin_1_decode)


# -- CP1252 --
_CP1252_DECODING_TABLE = (
    "\x00\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\x0c\r\x0e\x0f"
    "\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f"
    " !\"#$%&'()*+,-./0123456789:;<=>?"
    "@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_"
    "`abcdefghijklmnopqrstuvwxyz{|}~\x7f"
    "\u20ac\ufffe\u201a\u0192\u201e\u2026\u2020\u2021"
    "\u02c6\u2030\u0160\u2039\u0152\ufffe\u017d\ufffe"
    "\ufffe\u2018\u2019\u201c\u201d\u2022\u2013\u2014"
    "\u02dc\u2122\u0161\u203a\u0153\ufffe\u017e\u0178"
    "\xa0\xa1\xa2\xa3\xa4\xa5\xa6\xa7\xa8\xa9\xaa\xab\xac\xad\xae\xaf"
    "\xb0\xb1\xb2\xb3\xb4\xb5\xb6\xb7\xb8\xb9\xba\xbb\xbc\xbd\xbe\xbf"
    "\xc0\xc1\xc2\xc3\xc4\xc5\xc6\xc7\xc8\xc9\xca\xcb\xcc\xcd\xce\xcf"
    "\xd0\xd1\xd2\xd3\xd4\xd5\xd6\xd7\xd8\xd9\xda\xdb\xdc\xdd\xde\xdf"
    "\xe0\xe1\xe2\xe3\xe4\xe5\xe6\xe7\xe8\xe9\xea\xeb\xec\xed\xee\xef"
    "\xf0\xf1\xf2\xf3\xf4\xf5\xf6\xf7\xf8\xf9\xfa\xfb\xfc\xfd\xfe\xff"
)
_CP1252_ENCODING_TABLE = charmap_build(_CP1252_DECODING_TABLE)


def _cp1252_encode(input, errors="strict"):
    return charmap_encode(input, errors, _CP1252_ENCODING_TABLE)


def _cp1252_decode(input, errors="strict"):
    return charmap_decode(input, errors, _CP1252_DECODING_TABLE)


class _Cp1252IncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return _cp1252_encode(input, self.errors)[0]


class _Cp1252IncDecoder(IncrementalDecoder):
    def decode(self, input, final=False):
        return _cp1252_decode(input, self.errors)[0]


class _Cp1252StreamWriter(StreamWriter):
    encode = staticmethod(_cp1252_encode)


class _Cp1252StreamReader(StreamReader):
    decode = staticmethod(_cp1252_decode)


# -- unicode_escape / raw_unicode_escape / utf_7 --
class _UnicodeEscapeIncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return unicode_escape_encode(input, self.errors)[0]


class _UnicodeEscapeIncDecoder(IncrementalDecoder):
    def decode(self, input, final=False):
        return unicode_escape_decode(input, self.errors)[0]


class _UnicodeEscapeStreamWriter(StreamWriter):
    encode = staticmethod(unicode_escape_encode)


class _UnicodeEscapeStreamReader(StreamReader):
    decode = staticmethod(unicode_escape_decode)


class _RawUnicodeEscapeIncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return raw_unicode_escape_encode(input, self.errors)[0]


class _RawUnicodeEscapeIncDecoder(IncrementalDecoder):
    def decode(self, input, final=False):
        return raw_unicode_escape_decode(input, self.errors)[0]


class _RawUnicodeEscapeStreamWriter(StreamWriter):
    encode = staticmethod(raw_unicode_escape_encode)


class _RawUnicodeEscapeStreamReader(StreamReader):
    decode = staticmethod(raw_unicode_escape_decode)


class _Utf7IncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return utf_7_encode(input, self.errors)[0]


class _Utf7IncDecoder(BufferedIncrementalDecoder):
    _buffer_decode = staticmethod(utf_7_decode)


class _Utf7StreamWriter(StreamWriter):
    encode = staticmethod(utf_7_encode)


class _Utf7StreamReader(StreamReader):
    decode = staticmethod(utf_7_decode)


# -- Non-text transforms: rot_13, hex, base64, zlib --

_ROT13_TABLE = str.maketrans(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
    "NOPQRSTUVWXYZABCDEFGHIJKLMnopqrstuvwxyzabcdefghijklm",
)


def _rot13_encode(input, errors="strict"):
    if not isinstance(input, str):
        raise TypeError(
            "descriptor 'translate' for 'str' objects doesn't apply to a '%s' object"
            % type(input).__name__
        )
    if errors != "strict":
        raise ValueError("unsupported error handling %r" % (errors,))
    return (input.translate(_ROT13_TABLE), len(input))


def _rot13_decode(input, errors="strict"):
    return _rot13_encode(input, errors)


class _Rot13IncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return _rot13_encode(input, self.errors)[0]


class _Rot13IncDecoder(IncrementalDecoder):
    def decode(self, input, final=False):
        return _rot13_decode(input, self.errors)[0]


class _Rot13StreamWriter(StreamWriter):
    encode = staticmethod(_rot13_encode)


class _Rot13StreamReader(StreamReader):
    decode = staticmethod(_rot13_decode)


def _hex_encode(input, errors="strict"):
    import binascii

    if errors != "strict":
        raise ValueError("unsupported error handling %r" % (errors,))
    raw = _to_bytes(input)
    return (binascii.b2a_hex(raw), len(raw))


def _hex_decode(input, errors="strict"):
    import binascii

    if errors != "strict":
        raise ValueError("unsupported error handling %r" % (errors,))
    if isinstance(input, str):
        return (binascii.a2b_hex(input), len(input))
    raw = _to_bytes(input)
    return (binascii.a2b_hex(raw), len(raw))


class _HexIncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return _hex_encode(input, self.errors)[0]


class _HexIncDecoder(IncrementalDecoder):
    def decode(self, input, final=False):
        return _hex_decode(input, self.errors)[0]


class _HexStreamWriter(StreamWriter):
    encode = staticmethod(_hex_encode)
    charbuffertype = bytes


class _HexStreamReader(StreamReader):
    decode = staticmethod(_hex_decode)
    charbuffertype = bytes


def _base64_encode(input, errors="strict"):
    import base64

    if errors != "strict":
        raise ValueError("unsupported error handling %r" % (errors,))
    raw = _to_bytes(input)
    return (base64.encodebytes(raw), len(raw))


def _base64_decode(input, errors="strict"):
    import base64

    if errors != "strict":
        raise ValueError("unsupported error handling %r" % (errors,))
    raw = _to_bytes(input)
    return (base64.decodebytes(raw), len(raw))


class _Base64IncEncoder(IncrementalEncoder):
    def encode(self, input, final=False):
        return _base64_encode(input, self.errors)[0]


class _Base64IncDecoder(IncrementalDecoder):
    def decode(self, input, final=False):
        return _base64_decode(input, self.errors)[0]


class _Base64StreamWriter(StreamWriter):
    encode = staticmethod(_base64_encode)
    charbuffertype = bytes


class _Base64StreamReader(StreamReader):
    decode = staticmethod(_base64_decode)
    charbuffertype = bytes


def _zlib_encode(input, errors="strict"):
    import zlib

    if errors != "strict":
        raise ValueError("unsupported error handling %r" % (errors,))
    raw = _to_bytes(input)
    return (zlib.compress(raw), len(raw))


def _zlib_decode(input, errors="strict"):
    import zlib

    if errors != "strict":
        raise ValueError("unsupported error handling %r" % (errors,))
    raw = _to_bytes(input)
    return (zlib.decompress(raw), len(raw))


class _ZlibIncEncoder(IncrementalEncoder):
    def __init__(self, errors="strict"):
        import zlib

        if errors != "strict":
            raise ValueError("unsupported error handling %r" % (errors,))
        IncrementalEncoder.__init__(self, errors)
        self.compressobj = zlib.compressobj()

    def encode(self, input, final=False):
        raw = _to_bytes(input)
        if final:
            c = self.compressobj.compress(raw)
            return c + self.compressobj.flush()
        return self.compressobj.compress(raw)

    def reset(self):
        import zlib

        self.compressobj = zlib.compressobj()


class _ZlibIncDecoder(IncrementalDecoder):
    def __init__(self, errors="strict"):
        import zlib

        if errors != "strict":
            raise ValueError("unsupported error handling %r" % (errors,))
        IncrementalDecoder.__init__(self, errors)
        self.decompressobj = zlib.decompressobj()

    def decode(self, input, final=False):
        raw = _to_bytes(input)
        if final:
            c = self.decompressobj.decompress(raw)
            return c + self.decompressobj.flush()
        return self.decompressobj.decompress(raw)

    def reset(self):
        import zlib

        self.decompressobj = zlib.decompressobj()


class _ZlibStreamWriter(StreamWriter):
    encode = staticmethod(_zlib_encode)
    charbuffertype = bytes


class _ZlibStreamReader(StreamReader):
    decode = staticmethod(_zlib_decode)
    charbuffertype = bytes


# ── Codec Registry & Lookup ──────────────────────────────────────────────────

def _normalize_encoding(encoding):
    if isinstance(encoding, bytes):
        encoding = encoding.decode("ascii")
    chars = []
    punct = False
    for c in encoding:
        if c.isalnum() or c == ".":
            if punct and chars:
                chars.append("_")
            if c.isascii():
                chars.append(c.lower())
            else:
                chars.append(c)
            punct = False
        else:
            punct = True
    return "".join(chars)


_BUILTIN_CODECS = {}

def _add_builtin_codec(info, aliases):
    for alias in aliases:
        _BUILTIN_CODECS[_normalize_encoding(alias)] = info


_add_builtin_codec(
    CodecInfo(
        name="utf-8",
        encode=utf_8_encode,
        decode=lambda input, errors="strict": utf_8_decode(input, errors, True),
        incrementalencoder=_Utf8IncEncoder,
        incrementaldecoder=_Utf8IncDecoder,
        streamreader=_Utf8StreamReader,
        streamwriter=_Utf8StreamWriter,
    ),
    ["utf-8", "utf_8", "utf8", "u8", "utf", "utf8_ucs2", "utf8_ucs4", "cp65001"],
)

_add_builtin_codec(
    CodecInfo(
        name="utf-8-sig",
        encode=_utf_8_sig_encode,
        decode=_utf_8_sig_decode,
        incrementalencoder=_Utf8SigIncEncoder,
        incrementaldecoder=_Utf8SigIncDecoder,
        streamreader=_Utf8SigStreamReader,
        streamwriter=_Utf8SigStreamWriter,
    ),
    ["utf-8-sig", "utf_8_sig", "utf8_sig"],
)

_add_builtin_codec(
    CodecInfo(
        name="utf-16",
        encode=utf_16_encode,
        decode=lambda input, errors="strict": utf_16_decode(input, errors, True),
        incrementalencoder=_Utf16IncEncoder,
        incrementaldecoder=_Utf16IncDecoder,
        streamreader=_Utf16StreamReader,
        streamwriter=_Utf16StreamWriter,
    ),
    ["utf-16", "utf_16", "utf16", "u16"],
)

_add_builtin_codec(
    CodecInfo(
        name="utf-16-le",
        encode=utf_16_le_encode,
        decode=lambda input, errors="strict": utf_16_le_decode(input, errors, True),
        incrementalencoder=_Utf16LeIncEncoder,
        incrementaldecoder=_Utf16LeIncDecoder,
        streamreader=_Utf16LeStreamReader,
        streamwriter=_Utf16LeStreamWriter,
    ),
    ["utf-16-le", "utf_16_le", "utf16le", "utf_16le"],
)

_add_builtin_codec(
    CodecInfo(
        name="utf-16-be",
        encode=utf_16_be_encode,
        decode=lambda input, errors="strict": utf_16_be_decode(input, errors, True),
        incrementalencoder=_Utf16BeIncEncoder,
        incrementaldecoder=_Utf16BeIncDecoder,
        streamreader=_Utf16BeStreamReader,
        streamwriter=_Utf16BeStreamWriter,
    ),
    ["utf-16-be", "utf_16_be", "utf16be", "utf_16be"],
)

_add_builtin_codec(
    CodecInfo(
        name="utf-32",
        encode=utf_32_encode,
        decode=lambda input, errors="strict": utf_32_decode(input, errors, True),
        incrementalencoder=_Utf32IncEncoder,
        incrementaldecoder=_Utf32IncDecoder,
        streamreader=_Utf32StreamReader,
        streamwriter=_Utf32StreamWriter,
    ),
    ["utf-32", "utf_32", "utf32", "u32"],
)

_add_builtin_codec(
    CodecInfo(
        name="utf-32-le",
        encode=utf_32_le_encode,
        decode=lambda input, errors="strict": utf_32_le_decode(input, errors, True),
        incrementalencoder=_Utf32LeIncEncoder,
        incrementaldecoder=_Utf32LeIncDecoder,
        streamreader=_Utf32LeStreamReader,
        streamwriter=_Utf32LeStreamWriter,
    ),
    ["utf-32-le", "utf_32_le", "utf32le", "utf_32le"],
)

_add_builtin_codec(
    CodecInfo(
        name="utf-32-be",
        encode=utf_32_be_encode,
        decode=lambda input, errors="strict": utf_32_be_decode(input, errors, True),
        incrementalencoder=_Utf32BeIncEncoder,
        incrementaldecoder=_Utf32BeIncDecoder,
        streamreader=_Utf32BeStreamReader,
        streamwriter=_Utf32BeStreamWriter,
    ),
    ["utf-32-be", "utf_32_be", "utf32be", "utf_32be"],
)

_add_builtin_codec(
    CodecInfo(
        name="ascii",
        encode=ascii_encode,
        decode=ascii_decode,
        incrementalencoder=_AsciiIncEncoder,
        incrementaldecoder=_AsciiIncDecoder,
        streamreader=_AsciiStreamReader,
        streamwriter=_AsciiStreamWriter,
    ),
    ["ascii", "us-ascii", "646", "us", "iso646-us", "ansi_x3.4_1968", "csascii"],
)

_add_builtin_codec(
    CodecInfo(
        name="iso8859-1",
        encode=latin_1_encode,
        decode=latin_1_decode,
        incrementalencoder=_Latin1IncEncoder,
        incrementaldecoder=_Latin1IncDecoder,
        streamreader=_Latin1StreamReader,
        streamwriter=_Latin1StreamWriter,
    ),
    ["iso8859-1", "iso-8859-1", "latin-1", "latin1", "latin", "l1", "cp819", "8859"],
)

_add_builtin_codec(
    CodecInfo(
        name="cp1252",
        encode=_cp1252_encode,
        decode=_cp1252_decode,
        incrementalencoder=_Cp1252IncEncoder,
        incrementaldecoder=_Cp1252IncDecoder,
        streamreader=_Cp1252StreamReader,
        streamwriter=_Cp1252StreamWriter,
    ),
    ["cp1252", "windows-1252", "1252"],
)

_add_builtin_codec(
    CodecInfo(
        name="unicode-escape",
        encode=unicode_escape_encode,
        decode=unicode_escape_decode,
        incrementalencoder=_UnicodeEscapeIncEncoder,
        incrementaldecoder=_UnicodeEscapeIncDecoder,
        streamreader=_UnicodeEscapeStreamReader,
        streamwriter=_UnicodeEscapeStreamWriter,
    ),
    ["unicode-escape", "unicode_escape"],
)

_add_builtin_codec(
    CodecInfo(
        name="raw-unicode-escape",
        encode=raw_unicode_escape_encode,
        decode=raw_unicode_escape_decode,
        incrementalencoder=_RawUnicodeEscapeIncEncoder,
        incrementaldecoder=_RawUnicodeEscapeIncDecoder,
        streamreader=_RawUnicodeEscapeStreamReader,
        streamwriter=_RawUnicodeEscapeStreamWriter,
    ),
    ["raw-unicode-escape", "raw_unicode_escape"],
)

_add_builtin_codec(
    CodecInfo(
        name="utf-7",
        encode=utf_7_encode,
        decode=lambda input, errors="strict": utf_7_decode(input, errors, True),
        incrementalencoder=_Utf7IncEncoder,
        incrementaldecoder=_Utf7IncDecoder,
        streamreader=_Utf7StreamReader,
        streamwriter=_Utf7StreamWriter,
    ),
    ["utf-7", "utf_7", "utf7", "u7"],
)

_add_builtin_codec(
    CodecInfo(
        name="rot-13",
        encode=_rot13_encode,
        decode=_rot13_decode,
        incrementalencoder=_Rot13IncEncoder,
        incrementaldecoder=_Rot13IncDecoder,
        streamreader=_Rot13StreamReader,
        streamwriter=_Rot13StreamWriter,
        _is_text_encoding=False,
    ),
    ["rot-13", "rot_13", "rot13"],
)

_add_builtin_codec(
    CodecInfo(
        name="hex",
        encode=_hex_encode,
        decode=_hex_decode,
        incrementalencoder=_HexIncEncoder,
        incrementaldecoder=_HexIncDecoder,
        streamreader=_HexStreamReader,
        streamwriter=_HexStreamWriter,
        _is_text_encoding=False,
    ),
    ["hex", "hex-codec", "hex_codec"],
)

_add_builtin_codec(
    CodecInfo(
        name="base64",
        encode=_base64_encode,
        decode=_base64_decode,
        incrementalencoder=_Base64IncEncoder,
        incrementaldecoder=_Base64IncDecoder,
        streamreader=_Base64StreamReader,
        streamwriter=_Base64StreamWriter,
        _is_text_encoding=False,
    ),
    ["base64", "base-64", "base_64", "base64-codec", "base64_codec"],
)

_add_builtin_codec(
    CodecInfo(
        name="zlib",
        encode=_zlib_encode,
        decode=_zlib_decode,
        incrementalencoder=_ZlibIncEncoder,
        incrementaldecoder=_ZlibIncDecoder,
        streamreader=_ZlibStreamReader,
        streamwriter=_ZlibStreamWriter,
        _is_text_encoding=False,
    ),
    ["zlib", "zlib-codec", "zlib_codec", "zip"],
)


def _builtin_search_function(encoding):
    return _BUILTIN_CODECS.get(encoding)


_search_path = [_builtin_search_function]
_search_cache = {}


def register(search_function):
    if not callable(search_function):
        raise TypeError("argument must be callable")
    _search_path.append(search_function)


def unregister(search_function):
    try:
        _search_path.remove(search_function)
    except ValueError:
        return
    _search_cache.clear()


def lookup(encoding):
    if not isinstance(encoding, str):
        raise TypeError("lookup() argument must be str, not %s" % type(encoding).__name__)
    norm = _normalize_encoding(encoding)
    if norm in _search_cache:
        return _search_cache[norm]
    for func in list(_search_path):
        result = func(norm)
        if result is not None:
            if not isinstance(result, tuple) or len(result) != 4:
                raise TypeError("codec search functions must return 4-tuples")
            if not isinstance(result, CodecInfo):
                result = CodecInfo(result[0], result[1], result[2], result[3], name=norm)
            _search_cache[norm] = result
            return result
    raise LookupError("unknown encoding: %s" % encoding)


def encode(obj, encoding="utf-8", errors="strict"):
    encoder = lookup(encoding).encode
    res = encoder(obj, errors)
    if not isinstance(res, tuple) or len(res) != 2:
        raise TypeError("encoder must return a tuple (object, integer)")
    return res[0]


def decode(obj, encoding="utf-8", errors="strict"):
    decoder = lookup(encoding).decode
    res = decoder(obj, errors)
    if not isinstance(res, tuple) or len(res) != 2:
        raise TypeError("decoder must return a tuple (object, integer)")
    return res[0]


def getencoder(encoding):
    return lookup(encoding).encode


def getdecoder(encoding):
    return lookup(encoding).decode


def getincrementalencoder(encoding):
    encoder = lookup(encoding).incrementalencoder
    if encoder is None:
        raise LookupError(encoding)
    return encoder


def getincrementaldecoder(encoding):
    decoder = lookup(encoding).incrementaldecoder
    if decoder is None:
        raise LookupError(encoding)
    return decoder


def getreader(encoding):
    return lookup(encoding).streamreader


def getwriter(encoding):
    return lookup(encoding).streamwriter


def iterencode(iterator, encoding, errors="strict", **kwargs):
    encoder = getincrementalencoder(encoding)(errors, **kwargs)
    for input in iterator:
        output = encoder.encode(input)
        if output:
            yield output
    output = encoder.encode("", True)
    if output:
        yield output


def iterdecode(iterator, encoding, errors="strict", **kwargs):
    decoder = getincrementaldecoder(encoding)(errors, **kwargs)
    for input in iterator:
        output = decoder.decode(input)
        if output:
            yield output
    output = decoder.decode(b"", True)
    if output:
        yield output


def open(filename, mode="r", encoding=None, errors="strict", buffering=-1):
    path = os.fspath(filename)
    if encoding is not None and "b" not in mode:
        mode = mode + "b"
    file = builtins.open(path, mode, buffering)
    if encoding is None:
        return file
    try:
        info = lookup(encoding)
        srw = StreamReaderWriter(file, info.streamreader, info.streamwriter, errors)
        srw.encoding = encoding
        return srw
    except BaseException:
        file.close()
        raise


def EncodedFile(file, data_encoding, file_encoding=None, errors="strict"):
    if file_encoding is None:
        file_encoding = data_encoding
    data_info = lookup(data_encoding)
    file_info = lookup(file_encoding)
    sr = StreamRecoder(
        file,
        data_info.encode,
        data_info.decode,
        file_info.streamreader,
        file_info.streamwriter,
        errors,
    )
    sr.data_encoding = data_encoding
    sr.file_encoding = file_encoding
    return sr
