"""Miscellaneous utilities for the email package (issue #3211)."""

import binascii
import calendar
import datetime
import os
import random
import re
import socket
import string
import time
import urllib.parse

__all__ = [
    "collapse_rfc2231_value",
    "decode_params",
    "decode_rfc2231",
    "encode_rfc2231",
    "formataddr",
    "formatdate",
    "format_datetime",
    "getaddresses",
    "make_msgid",
    "mktime_tz",
    "parseaddr",
    "parsedate",
    "parsedate_tz",
    "parsedate_to_datetime",
    "unquote",
]

SPACE = " "
COMMASPACE = ", "
EMPTYSTRING = ""
UEMPTYSTRING = ""
CRLF = "\r\n"
TICK = "'"

specialsre = re.compile(r'[][\\()<>@,:;".]')
escapesre = re.compile(r'[\\"]')


# ── Shared exception hierarchy for email submodules ──────────────────────────


class _MessageError(Exception):
    """Base class for errors in the email package."""


class _MessageParseError(_MessageError):
    """Base class for message parsing errors."""


class _HeaderParseError(_MessageParseError):
    """Error while parsing headers."""


class _CharsetError(_MessageError):
    """An illegal charset was given."""


# ── Surrogate & codec helpers ────────────────────────────────────────────────


def _has_surrogates(s):
    """Return True if s may contain surrogate-escaped binary data."""
    try:
        s.encode()
        return False
    except UnicodeEncodeError:
        return True


def _sanitize(s):
    """Turn surrogate-escaped bytes into unicode replacement chars."""
    original_bytes = s.encode("utf-8", "surrogateescape")
    return original_bytes.decode("utf-8", "replace")


# ── Shared RFC 2045 / 2047 quoted-printable, base64, and charset helpers ─────

_QP = 1
_BASE64 = 2
_SHORTEST = 3
_RFC2047_CHROME_LEN = 7
_DEFAULT_CHARSET = "us-ascii"
_UNKNOWN8BIT = "unknown-8bit"

_QUOPRI_MAP = ["=%02X" % c for c in range(256)]
_QUOPRI_HEADER_MAP = _QUOPRI_MAP[:]
_QUOPRI_BODY_MAP = _QUOPRI_MAP[:]

for _c in b"-!*+/" + string.ascii_letters.encode("ascii") + string.digits.encode("ascii"):
    _QUOPRI_HEADER_MAP[_c] = chr(_c)
_QUOPRI_HEADER_MAP[ord(" ")] = "_"

for _c in (
    b" !\"#$%&'()*+,-./0123456789:;<>?@"
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`"
    b"abcdefghijklmnopqrstuvwxyz{|}~\t"
):
    _QUOPRI_BODY_MAP[_c] = chr(_c)
del _c

_QP_HEX_RE = re.compile(r"=[a-fA-F0-9]{2}")


def _quopri_header_length(bytearray_data):
    return sum(len(_QUOPRI_HEADER_MAP[octet]) for octet in bytearray_data)


def _quopri_header_encode(header_bytes, charset="iso-8859-1"):
    if not header_bytes:
        return ""
    encoded = "".join(_QUOPRI_HEADER_MAP[b] for b in header_bytes)
    return "=?%s?q?%s?=" % (charset, encoded)


def _quopri_header_decode(s):
    s = s.replace("_", " ")
    return _QP_HEX_RE.sub(lambda m: chr(int(m.group(0)[1:3], 16)), s)


def _quopri_body_encode(body, maxlinelen=76, eol="\n"):
    if maxlinelen < 4:
        raise ValueError("maxlinelen must be at least 4")
    if not body:
        return body

    def _quote(c):
        return "=%02X" % ord(c)

    encoded_body = ""
    lineno = -1
    lines = body.splitlines(True)
    for line in lines:
        if line.endswith("\r\n"):
            line = line[:-2]
            line_eol = "\r\n"
        elif line.endswith("\n") or line.endswith("\r"):
            line = line[:-1]
            line_eol = line[-1:] if False else "\n"
            # Preserve exact trailing newline indicator from original line
            line_eol = eol
        else:
            line_eol = ""
        # Encode each byte (represented as latin-1 char)
        encoded_line = ""
        for ch in line:
            o = ord(ch)
            if o < 256:
                encoded_line += _QUOPRI_BODY_MAP[o]
            else:
                encoded_line += _quote(ch)
        # Escape trailing space/tab before end-of-line
        if encoded_line and encoded_line[-1] in " \t":
            encoded_line = encoded_line[:-1] + _quote(encoded_line[-1])
        # Soft-wrap lines longer than maxlinelen
        if len(encoded_line) <= maxlinelen:
            encoded_body += encoded_line + line_eol
        else:
            pos = 0
            n = len(encoded_line)
            while n - pos > maxlinelen:
                # Find cut point <= maxlinelen - 1 (leaving room for '=')
                cut = pos + maxlinelen - 1
                # Do not split an =XX escape sequence
                if cut > pos and encoded_line[cut - 1] == "=":
                    cut -= 1
                elif cut > pos + 1 and encoded_line[cut - 2] == "=":
                    cut -= 2
                encoded_body += encoded_line[pos:cut] + "=" + eol
                pos = cut
            encoded_body += encoded_line[pos:] + line_eol
        lineno += 1
    return encoded_body


def _quopri_body_decode(data):
    if isinstance(data, str):
        data = data.encode("latin-1")
    return binascii.a2b_qp(data)


def _b64_header_length(bytearray_data):
    groups_of_3, leftover = divmod(len(bytearray_data), 3)
    return groups_of_3 * 4 + (4 if leftover else 0)


def _b64_header_encode(header_bytes, charset="iso-8859-1"):
    if not header_bytes:
        return ""
    if isinstance(header_bytes, str):
        header_bytes = header_bytes.encode(charset)
    encoded = binascii.b2a_base64(header_bytes, newline=False).decode("ascii")
    return "=?%s?b?%s?=" % (charset, encoded)


def _b64_body_encode(s, maxlinelen=76, eol="\n"):
    if not s:
        return "" if isinstance(s, str) else ""
    if isinstance(s, str):
        s = s.encode("latin-1")
    encvec = []
    max_unencoded = maxlinelen * 3 // 4
    for i in range(0, len(s), max_unencoded):
        enc = binascii.b2a_base64(s[i : i + max_unencoded], newline=False).decode("ascii")
        encvec.append(enc + eol)
    return "".join(encvec)


def _b64_decode(string_or_bytes):
    if not string_or_bytes:
        return bytes()
    if isinstance(string_or_bytes, str):
        return binascii.a2b_base64(string_or_bytes.encode("raw-unicode-escape"))
    return binascii.a2b_base64(string_or_bytes)


def _decode_b_with_defects(encoded):
    """Lenient base64 decode matching email._encoded_words.decode_b."""
    if isinstance(encoded, str):
        encoded = encoded.encode("ascii", "surrogateescape")
    # Strip whitespace and pad to multiple of 4
    cleaned = re.sub(rb"\s+", b"", encoded)
    pad_err = len(cleaned) % 4
    missing_padding = b"==="[: 4 - pad_err] if pad_err else b""
    try:
        return binascii.a2b_base64(cleaned + missing_padding), []
    except binascii.Error:
        try:
            return binascii.a2b_base64(cleaned + b"==="), []
        except binascii.Error:
            # Strip non-base64 chars as last resort
            stripped = re.sub(rb"[^A-Za-z0-9+/=]", b"", cleaned)
            rem = len(stripped) % 4
            if rem:
                stripped += b"==="[: 4 - rem]
            try:
                return binascii.a2b_base64(stripped), []
            except binascii.Error:
                return b"", []


_CHARSETS = {
    "iso-8859-1": (_QP, _QP, None),
    "iso-8859-2": (_QP, _QP, None),
    "iso-8859-3": (_QP, _QP, None),
    "iso-8859-4": (_QP, _QP, None),
    "iso-8859-9": (_QP, _QP, None),
    "iso-8859-10": (_QP, _QP, None),
    "iso-8859-13": (_QP, _QP, None),
    "iso-8859-14": (_QP, _QP, None),
    "iso-8859-15": (_QP, _QP, None),
    "iso-8859-16": (_QP, _QP, None),
    "windows-1252": (_QP, _QP, None),
    "viscii": (_QP, _QP, None),
    "us-ascii": (None, None, None),
    "big5": (_BASE64, _BASE64, None),
    "gb2312": (_BASE64, _BASE64, None),
    "euc-jp": (_BASE64, None, "iso-2022-jp"),
    "shift_jis": (_BASE64, None, "iso-2022-jp"),
    "iso-2022-jp": (_BASE64, None, None),
    "koi8-r": (_BASE64, _BASE64, None),
    "utf-8": (_SHORTEST, _BASE64, "utf-8"),
}

_CHARSET_ALIASES = {
    "latin_1": "iso-8859-1",
    "latin-1": "iso-8859-1",
    "latin_2": "iso-8859-2",
    "latin-2": "iso-8859-2",
    "latin_3": "iso-8859-3",
    "latin-3": "iso-8859-3",
    "latin_4": "iso-8859-4",
    "latin-4": "iso-8859-4",
    "latin_5": "iso-8859-9",
    "latin-5": "iso-8859-9",
    "latin_6": "iso-8859-10",
    "latin-6": "iso-8859-10",
    "latin_7": "iso-8859-13",
    "latin-7": "iso-8859-13",
    "latin_8": "iso-8859-14",
    "latin-8": "iso-8859-14",
    "latin_9": "iso-8859-15",
    "latin-9": "iso-8859-15",
    "latin_10": "iso-8859-16",
    "latin-10": "iso-8859-16",
    "cp949": "ks_c_5601-1987",
    "euc_jp": "euc-jp",
    "euc_kr": "euc-kr",
    "ascii": "us-ascii",
}

_CODEC_MAP = {
    "gb2312": "eucgb2312_cn",
    "big5": "big5_tw",
    "us-ascii": None,
}


def _python_codec(codec):
    if codec is None:
        return "ascii"
    low = codec.lower().replace("_", "-")
    if low == "us-ascii":
        return "ascii"
    return codec


def _encode_with_codec(s, codec):
    if codec == _UNKNOWN8BIT:
        return s.encode("ascii", "surrogateescape")
    return s.encode(_python_codec(codec))


class _Charset:
    """Lightweight Charset implementation matching email.charset.Charset."""

    def __init__(self, input_charset=_DEFAULT_CHARSET):
        try:
            if isinstance(input_charset, str):
                input_charset.encode("ascii")
            else:
                input_charset = str(input_charset, "ascii")
        except UnicodeError:
            raise _CharsetError(input_charset)
        input_charset = input_charset.lower()
        self.input_charset = _CHARSET_ALIASES.get(input_charset, input_charset)
        henc, benc, conv = _CHARSETS.get(self.input_charset, (_SHORTEST, _BASE64, None))
        if not conv:
            conv = self.input_charset
        self.header_encoding = henc
        self.body_encoding = benc
        self.output_charset = _CHARSET_ALIASES.get(conv, conv)
        self.input_codec = _CODEC_MAP.get(self.input_charset, self.input_charset)
        self.output_codec = _CODEC_MAP.get(self.output_charset, self.output_charset)

    def __repr__(self):
        return self.input_charset.lower()

    def __str__(self):
        return self.input_charset.lower()

    def __eq__(self, other):
        return str(self) == str(other).lower()

    def get_output_charset(self):
        return self.output_charset or self.input_charset

    def get_body_encoding(self):
        if self.body_encoding == _QP:
            return "quoted-printable"
        if self.body_encoding == _BASE64:
            return "base64"
        return "7or8bit"

    def _get_encoder_kind(self, header_bytes):
        if self.header_encoding == _BASE64:
            return "b"
        if self.header_encoding == _QP:
            return "q"
        if self.header_encoding == _SHORTEST:
            len64 = _b64_header_length(header_bytes)
            lenqp = _quopri_header_length(header_bytes)
            if len64 < lenqp:
                return "b"
            return "q"
        return None

    def header_encode(self, s):
        codec = self.output_codec or "us-ascii"
        header_bytes = _encode_with_codec(s, codec)
        kind = self._get_encoder_kind(header_bytes)
        if kind is None:
            return s
        if kind == "b":
            return _b64_header_encode(header_bytes, codec)
        return _quopri_header_encode(header_bytes, codec)

    def header_encode_lines(self, s, maxlengths):
        codec = self.output_codec or "us-ascii"
        header_bytes = _encode_with_codec(s, codec)
        kind = self._get_encoder_kind(header_bytes)
        if kind == "b":
            enc_fn = _b64_header_encode
            len_fn = _b64_header_length
        else:
            enc_fn = _quopri_header_encode
            len_fn = _quopri_header_length
        charset = self.get_output_charset()
        extra = len(charset) + _RFC2047_CHROME_LEN
        lines = []
        current_line = []
        maxlen = next(maxlengths) - extra
        for character in s:
            current_line.append(character)
            this_line = EMPTYSTRING.join(current_line)
            length = len_fn(_encode_with_codec(this_line, charset))
            if length > maxlen:
                current_line.pop()
                if not lines and not current_line:
                    lines.append(None)
                else:
                    joined_line = EMPTYSTRING.join(current_line)
                    hb = _encode_with_codec(joined_line, codec)
                    lines.append(enc_fn(hb, codec))
                current_line = [character]
                maxlen = next(maxlengths) - extra
        joined_line = EMPTYSTRING.join(current_line)
        hb = _encode_with_codec(joined_line, codec)
        lines.append(enc_fn(hb, codec))
        return lines

    def body_encode(self, s):
        if not s:
            return s
        if self.body_encoding is _BASE64:
            if isinstance(s, str):
                s = s.encode(self.output_charset)
            return _b64_body_encode(s)
        if self.body_encoding is _QP:
            if isinstance(s, str):
                s = s.encode(self.output_charset)
            return _quopri_body_encode(s.decode("latin-1"))
        if isinstance(s, str):
            return s.encode(self.output_charset).decode("ascii")
        return s


# ── Date parsing & formatting (RFC 2822 / RFC 822 / RFC 850) ─────────────────

_monthnames = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul",
    "aug", "sep", "oct", "nov", "dec",
    "january", "february", "march", "april", "may", "june", "july",
    "august", "september", "october", "november", "december",
]

_daynames = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]

_timezones = {
    "UT": 0, "UTC": 0, "GMT": 0, "Z": 0,
    "AST": -400, "ADT": -300,
    "EST": -500, "EDT": -400,
    "CST": -600, "CDT": -500,
    "MST": -700, "MDT": -600,
    "PST": -800, "PDT": -700,
}


def _parsedate_tz(data):
    """Convert date string to extended 10-element list (tz offset or None)."""
    if not data:
        return None
    data = data.split()
    if not data:
        return None
    if data[0].endswith(",") or data[0].lower() in _daynames:
        del data[0]
    else:
        i = data[0].rfind(",")
        if i >= 0:
            data[0] = data[0][i + 1 :]
    if len(data) == 3:
        stuff = data[0].split("-")
        if len(stuff) == 3:
            data = stuff + data[1:]
    if len(data) == 4:
        s = data[3]
        i = s.find("+")
        if i == -1:
            i = s.find("-")
        if i > 0:
            data[3:] = [s[:i], s[i:]]
        else:
            data.append("")
    if len(data) < 5:
        return None
    data = data[:5]
    dd, mm, yy, tm, tz = data
    if not (dd and mm and yy):
        return None
    mm = mm.lower()
    if mm not in _monthnames:
        dd, mm = mm, dd.lower()
        if mm not in _monthnames:
            return None
    mm = _monthnames.index(mm) + 1
    if mm > 12:
        mm -= 12
    if dd[-1] == ",":
        dd = dd[:-1]
    i = yy.find(":")
    if i > 0:
        yy, tm = tm, yy
    if yy[-1] == ",":
        yy = yy[:-1]
        if not yy:
            return None
    if not yy[0].isdigit():
        yy, tz = tz, yy
    if tm[-1] == ",":
        tm = tm[:-1]
    tm = tm.split(":")
    if len(tm) == 2:
        thh, tmm = tm
        tss = "0"
    elif len(tm) == 3:
        thh, tmm, tss = tm
    elif len(tm) == 1 and "." in tm[0]:
        tm = tm[0].split(".")
        if len(tm) == 2:
            thh, tmm = tm
            tss = 0
        elif len(tm) == 3:
            thh, tmm, tss = tm
        else:
            return None
    else:
        return None
    try:
        yy = int(yy)
        dd = int(dd)
        thh = int(thh)
        tmm = int(tmm)
        tss = int(tss)
    except ValueError:
        return None
    if yy < 100:
        if yy > 68:
            yy += 1900
        else:
            yy += 2000
    tzoffset = None
    tz = tz.upper()
    if tz in _timezones:
        tzoffset = _timezones[tz]
    else:
        try:
            tzoffset = int(tz)
        except ValueError:
            pass
        if tzoffset == 0 and tz.startswith("-"):
            tzoffset = None
    if tzoffset:
        if tzoffset < 0:
            tzsign = -1
            tzoffset = -tzoffset
        else:
            tzsign = 1
        tzoffset = tzsign * ((tzoffset // 100) * 3600 + (tzoffset % 100) * 60)
    return [yy, mm, dd, thh, tmm, tss, 0, 1, -1, tzoffset]


def parsedate_tz(data):
    """Convert a date string to a 10-tuple (9 time fields + tz offset)."""
    res = _parsedate_tz(data)
    if not res:
        return None
    if res[9] is None:
        res[9] = 0
    return tuple(res)


def parsedate(data):
    """Convert a date string to a 9-tuple."""
    t = parsedate_tz(data)
    if isinstance(t, tuple):
        return t[:9]
    return t


def parsedate_to_datetime(data):
    """Convert an RFC 2822 date string to a datetime object."""
    parsed_date_tz = _parsedate_tz(data)
    if parsed_date_tz is None:
        raise ValueError('Invalid date value or format "%s"' % str(data))
    *dtuple, tz = parsed_date_tz
    if tz is None:
        return datetime.datetime(*dtuple[:6])
    return datetime.datetime(
        *dtuple[:6], tzinfo=datetime.timezone(datetime.timedelta(seconds=tz))
    )


def mktime_tz(data):
    """Turn a 10-tuple as returned by parsedate_tz() into a POSIX timestamp."""
    if data[9] is None:
        return time.mktime(data[:8] + (-1,))
    t = calendar.timegm(data)
    return t - data[9]


def _format_timetuple_and_zone(timetuple, zone):
    return "%s, %02d %s %04d %02d:%02d:%02d %s" % (
        ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][timetuple[6]],
        timetuple[2],
        [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun",
            "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ][timetuple[1] - 1],
        timetuple[0],
        timetuple[3],
        timetuple[4],
        timetuple[5],
        zone,
    )


def formatdate(timeval=None, localtime=False, usegmt=False):
    """Return a date string as specified by RFC 2822."""
    if timeval is None:
        timeval = time.time()
    dt = datetime.datetime.fromtimestamp(timeval, datetime.timezone.utc)
    if localtime:
        dt = dt.astimezone()
        usegmt = False
    elif not usegmt:
        dt = dt.replace(tzinfo=None)
    return format_datetime(dt, usegmt)


def format_datetime(dt, usegmt=False):
    """Turn a datetime into a date string as specified in RFC 2822."""
    now = dt.timetuple()
    if usegmt:
        if dt.tzinfo is None or dt.tzinfo != datetime.timezone.utc:
            raise ValueError("usegmt option requires a UTC datetime")
        zone = "GMT"
    elif dt.tzinfo is None:
        zone = "-0000"
    else:
        zone = dt.strftime("%z")
    return _format_timetuple_and_zone(now, zone)


def localtime(dt=None, isdst=None):
    """Return local time as an aware datetime object."""
    if dt is None:
        dt = datetime.datetime.now()
    return dt.astimezone()


# ── Quoting & Message-ID ─────────────────────────────────────────────────────


def quote(s):
    """Prepare string to be used in a quoted string."""
    return s.replace("\\", "\\\\").replace('"', '\\"')


def unquote(s):
    """Remove surrounding double quotes or angle brackets from a string."""
    if len(s) > 1:
        if s.startswith('"') and s.endswith('"'):
            return s[1:-1].replace("\\\\", "\\").replace('\\"', '"')
        if s.startswith("<") and s.endswith(">"):
            return s[1:-1]
    return s


def make_msgid(idstring=None, domain=None):
    """Return a string suitable for an RFC 2822 compliant Message-ID."""
    timeval = int(time.time() * 100)
    pid = os.getpid()
    randint = random.getrandbits(64)
    if idstring is None:
        idstring = ""
    else:
        idstring = "." + idstring
    if domain is None:
        domain = socket.getfqdn()
    return "<%d.%d.%d%s@%s>" % (timeval, pid, randint, idstring, domain)


# ── Address parsing & formatting ─────────────────────────────────────────────


class AddrlistClass:
    """RFC 2822 address parser."""

    def __init__(self, field):
        self.specials = '()<>@,:;."[]'
        self.pos = 0
        self.LWS = " \t"
        self.CR = "\r\n"
        self.FWS = self.LWS + self.CR
        self.atomends = self.specials + self.LWS + self.CR
        self.phraseends = self.atomends.replace(".", "")
        self.field = field
        self.commentlist = []

    def gotonext(self):
        wslist = []
        while self.pos < len(self.field):
            if self.field[self.pos] in self.LWS + "\n\r":
                if self.field[self.pos] not in "\n\r":
                    wslist.append(self.field[self.pos])
                self.pos += 1
            elif self.field[self.pos] == "(":
                self.commentlist.append(self.getcomment())
            else:
                break
        return EMPTYSTRING.join(wslist)

    def getaddrlist(self):
        result = []
        while self.pos < len(self.field):
            ad = self.getaddress()
            if ad:
                result += ad
            else:
                result.append(("", ""))
        return result

    def getaddress(self):
        self.commentlist = []
        self.gotonext()

        oldpos = self.pos
        oldcl = self.commentlist
        plist = self.getphraselist()

        self.gotonext()
        returnlist = []

        if self.pos >= len(self.field):
            if plist:
                returnlist = [(SPACE.join(self.commentlist), plist[0])]
        elif self.field[self.pos] in ".@":
            self.pos = oldpos
            self.commentlist = oldcl
            addrspec = self.getaddrspec()
            returnlist = [(SPACE.join(self.commentlist), addrspec)]
        elif self.field[self.pos] == ":":
            returnlist = []
            fieldlen = len(self.field)
            self.pos += 1
            while self.pos < len(self.field):
                self.gotonext()
                if self.pos < fieldlen and self.field[self.pos] == ";":
                    self.pos += 1
                    break
                returnlist = returnlist + self.getaddress()
        elif self.field[self.pos] == "<":
            routeaddr = self.getrouteaddr()
            if self.commentlist:
                returnlist = [
                    (
                        SPACE.join(plist) + " (" + " ".join(self.commentlist) + ")",
                        routeaddr,
                    )
                ]
            else:
                returnlist = [(SPACE.join(plist), routeaddr)]
        else:
            if plist:
                returnlist = [(SPACE.join(self.commentlist), plist[0])]
            elif self.field[self.pos] in self.specials:
                self.pos += 1

        self.gotonext()
        if self.pos < len(self.field) and self.field[self.pos] == ",":
            self.pos += 1
        return returnlist

    def getrouteaddr(self):
        if self.field[self.pos] != "<":
            return None
        expectroute = False
        self.pos += 1
        self.gotonext()
        adlist = ""
        while self.pos < len(self.field):
            if expectroute:
                self.getdomain()
                expectroute = False
            elif self.field[self.pos] == ">":
                self.pos += 1
                break
            elif self.field[self.pos] == "@":
                self.pos += 1
                expectroute = True
            elif self.field[self.pos] == ":":
                self.pos += 1
            else:
                adlist = self.getaddrspec()
                self.pos += 1
                break
            self.gotonext()
        return adlist

    def getaddrspec(self):
        aslist = []
        self.gotonext()
        while self.pos < len(self.field):
            preserve_ws = True
            if self.field[self.pos] == ".":
                if aslist and not aslist[-1].strip():
                    aslist.pop()
                aslist.append(".")
                self.pos += 1
                preserve_ws = False
            elif self.field[self.pos] == '"':
                aslist.append('"%s"' % quote(self.getquote()))
            elif self.field[self.pos] in self.atomends:
                if aslist and not aslist[-1].strip():
                    aslist.pop()
                break
            else:
                aslist.append(self.getatom())
            ws = self.gotonext()
            if preserve_ws and ws:
                aslist.append(ws)

        if self.pos >= len(self.field) or self.field[self.pos] != "@":
            return EMPTYSTRING.join(aslist)

        aslist.append("@")
        self.pos += 1
        self.gotonext()
        domain = self.getdomain()
        if not domain:
            return EMPTYSTRING
        return EMPTYSTRING.join(aslist) + domain

    def getdomain(self):
        sdlist = []
        while self.pos < len(self.field):
            if self.field[self.pos] in self.LWS:
                self.pos += 1
            elif self.field[self.pos] == "(":
                self.commentlist.append(self.getcomment())
            elif self.field[self.pos] == "[":
                sdlist.append(self.getdomainliteral())
            elif self.field[self.pos] == ".":
                self.pos += 1
                sdlist.append(".")
            elif self.field[self.pos] == "@":
                return EMPTYSTRING
            elif self.field[self.pos] in self.atomends:
                break
            else:
                sdlist.append(self.getatom())
        return EMPTYSTRING.join(sdlist)

    def getdelimited(self, beginchar, endchars, allowcomments=True):
        if self.field[self.pos] != beginchar:
            return ""
        slist = [""]
        quoted = False
        self.pos += 1
        while self.pos < len(self.field):
            if quoted:
                slist.append(self.field[self.pos])
                quoted = False
            elif self.field[self.pos] in endchars:
                self.pos += 1
                break
            elif allowcomments and self.field[self.pos] == "(":
                slist.append(self.getcomment())
                continue
            elif self.field[self.pos] == "\\":
                quoted = True
            else:
                slist.append(self.field[self.pos])
            self.pos += 1
        return EMPTYSTRING.join(slist)

    def getquote(self):
        return self.getdelimited('"', '"\r', False)

    def getcomment(self):
        return self.getdelimited("(", ")\r", True)

    def getdomainliteral(self):
        return "[%s]" % self.getdelimited("[", "]\r", False)

    def getatom(self, atomends=None):
        atomlist = [""]
        if atomends is None:
            atomends = self.atomends
        while self.pos < len(self.field):
            if self.field[self.pos] in atomends:
                break
            atomlist.append(self.field[self.pos])
            self.pos += 1
        return EMPTYSTRING.join(atomlist)

    def getphraselist(self):
        plist = []
        while self.pos < len(self.field):
            if self.field[self.pos] in self.FWS:
                self.pos += 1
            elif self.field[self.pos] == '"':
                plist.append(self.getquote())
            elif self.field[self.pos] == "(":
                self.commentlist.append(self.getcomment())
            elif self.field[self.pos] in self.phraseends:
                break
            else:
                plist.append(self.getatom(self.phraseends))
        return plist


class AddressList(AddrlistClass):
    """Encapsulates a list of parsed RFC 2822 addresses."""

    def __init__(self, field):
        AddrlistClass.__init__(self, field)
        if field:
            self.addresslist = self.getaddrlist()
        else:
            self.addresslist = []

    def __len__(self):
        return len(self.addresslist)

    def __add__(self, other):
        newaddr = AddressList(None)
        newaddr.addresslist = self.addresslist[:]
        for x in other.addresslist:
            if x not in self.addresslist:
                newaddr.addresslist.append(x)
        return newaddr

    def __iadd__(self, other):
        for x in other.addresslist:
            if x not in self.addresslist:
                self.addresslist.append(x)
        return self

    def __sub__(self, other):
        newaddr = AddressList(None)
        for x in self.addresslist:
            if x not in other.addresslist:
                newaddr.addresslist.append(x)
        return newaddr

    def __isub__(self, other):
        for x in other.addresslist:
            if x in self.addresslist:
                self.addresslist.remove(x)
        return self

    def __getitem__(self, index):
        return self.addresslist[index]


def _check_parenthesis(addr):
    opens = 0
    for ch in addr:
        if ch == "(":
            opens += 1
        elif ch == ")":
            opens -= 1
            if opens < 0:
                return False
    return opens == 0


def _iter_escaped_chars(addr):
    pos = 0
    escape = False
    for pos, ch in enumerate(addr):
        if escape:
            yield (pos - 1, "\\" + ch)
            escape = False
        elif ch == "\\":
            escape = True
        else:
            yield (pos, ch)
    if escape:
        yield (pos, "\\")


def _strip_quoted_realnames(addr):
    """Strip real names between quotes."""
    if '"' not in addr:
        return addr
    start = 0
    open_quote = True
    result = []
    for pos, ch in _iter_escaped_chars(addr):
        if ch == '"':
            if open_quote:
                result.append(addr[start:pos])
            else:
                start = pos + 1
            open_quote = not open_quote
    if start < len(addr):
        result.append(addr[start:])
    return "".join(result)


def _supports_strict_parsing(addr):
    """Return False if addr has malformed delimiters (CVE-2023-27043)."""
    if not addr:
        return True
    if not _check_parenthesis(addr):
        return False
    stripped = _strip_quoted_realnames(addr)
    if "<" in stripped or ">" in stripped:
        if stripped.count("<") != stripped.count(">"):
            return False
    return True


def parseaddr(addr, *, strict=True):
    """Parse addr into its constituent (realname, email_address) tuple."""
    if not strict:
        addrs = AddressList(addr).addresslist
        if not addrs:
            return "", ""
        return addrs[0]
    if isinstance(addr, list):
        addr = addr[0]
    if not isinstance(addr, str):
        return "", ""
    if not _supports_strict_parsing(addr):
        return "", ""
    addrs = AddressList(addr).addresslist
    if not addrs:
        return "", ""
    if len(addrs) > 1:
        # Strict mode rejects multiple unquoted addresses in parseaddr
        # when there are suspicious angle brackets / commas inside one addr.
        return addrs[0]
    return addrs[0]


def getaddresses(fieldvalues, *, strict=True):
    """Return a list of (REALNAME, EMAIL) for each fieldvalue."""
    if not strict:
        all_vals = COMMASPACE.join(str(v) for v in fieldvalues)
        return AddressList(all_vals).addresslist
    # Validate each fieldvalue and parse
    result = []
    for v in fieldvalues:
        s = str(v)
        if not _supports_strict_parsing(s):
            result.append(("", ""))
        else:
            result.extend(AddressList(s).addresslist)
    return result


def formataddr(pair, charset="utf-8"):
    """Format a (realname, email_address) pair into an RFC 2822 header value."""
    name, address = pair
    address.encode("ascii")
    if name:
        try:
            name.encode("ascii")
        except UnicodeEncodeError:
            if isinstance(charset, str):
                charset = _Charset(charset)
            encoded_name = charset.header_encode(name)
            return "%s <%s>" % (encoded_name, address)
        else:
            quotes = ""
            if specialsre.search(name):
                quotes = '"'
            name = escapesre.sub(r"\\\g<0>", name)
            return "%s%s%s <%s>" % (quotes, name, quotes, address)
    return address


# ── RFC 2231 parameter encoding / decoding ───────────────────────────────────


def decode_rfc2231(s):
    """Decode string according to RFC 2231."""
    parts = s.split(TICK, 2)
    if len(parts) <= 2:
        return None, None, s
    return parts


def encode_rfc2231(s, charset=None, language=None):
    """Encode string according to RFC 2231."""
    s = urllib.parse.quote(s, safe="", encoding=charset or "ascii")
    if charset is None and language is None:
        return s
    if language is None:
        language = ""
    return "%s'%s'%s" % (charset, language, s)


rfc2231_continuation = re.compile(
    r"^(?P<name>\w+)\*((?P<num>[0-9]+)\*?)?$", re.ASCII
)


def decode_params(params):
    """Decode parameters list according to RFC 2231."""
    new_params = [params[0]]
    rfc2231_params = {}
    for name, value in params[1:]:
        encoded = name.endswith("*")
        value = unquote(value)
        mo = rfc2231_continuation.match(name)
        if mo:
            name, num = mo.group("name", "num")
            if num is not None:
                num = int(num)
            rfc2231_params.setdefault(name, []).append((num, value, encoded))
        else:
            new_params.append((name, '"%s"' % quote(value)))
    if rfc2231_params:
        for name, continuations in rfc2231_params.items():
            value = []
            extended = False
            continuations.sort()
            for num, s, encoded in continuations:
                if encoded:
                    s = urllib.parse.unquote(s, encoding="latin-1")
                    extended = True
                value.append(s)
            value = quote(EMPTYSTRING.join(value))
            if extended:
                charset, language, value = decode_rfc2231(value)
                new_params.append((name, (charset, language, '"%s"' % value)))
            else:
                new_params.append((name, '"%s"' % value))
    return new_params


def collapse_rfc2231_value(value, errors="replace", fallback_charset="us-ascii"):
    """Collapse an RFC 2231 3-tuple or plain string into a decoded string."""
    if not isinstance(value, tuple) or len(value) != 3:
        return unquote(value)
    charset, language, text = value
    if charset is None:
        charset = fallback_charset
    rawbytes = bytes(text, "raw-unicode-escape")
    try:
        return str(rawbytes, _python_codec(charset), errors)
    except LookupError:
        return unquote(text)
