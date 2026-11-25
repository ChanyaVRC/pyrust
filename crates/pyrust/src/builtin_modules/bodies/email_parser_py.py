"""A parser of RFC 2822 and MIME email messages (issue #3211)."""

import re
from io import StringIO

from email.message import Message
from email.policy import compat32

__all__ = [
    "Parser",
    "HeaderParser",
    "BytesParser",
    "BytesHeaderParser",
    "FeedParser",
    "BytesFeedParser",
]

_HEADER_RE = re.compile(r"^(From |[\x21-\x39\x3b-\x7e]*:|[\t ])")
_EMPTY_LINE_RE = re.compile(r"^(\r\n|\r|\n)$")


def _make_message(factory, policy):
    if factory is None:
        if policy.message_factory is None:
            return Message(policy=policy)
        return policy.message_factory(policy=policy)
    try:
        return factory(policy=policy)
    except TypeError:
        return factory()


def _split_lines_keepends(text):
    """Split text into lines preserving exact \\r\\n, \\n, or \\r endings."""
    if not text:
        return []
    lines = []
    i = 0
    n = len(text)
    while i < n:
        j = i
        while j < n and text[j] not in "\r\n":
            j += 1
        if j < n:
            if text[j] == "\r" and j + 1 < n and text[j + 1] == "\n":
                j += 2
            else:
                j += 1
        lines.append(text[i:j])
        i = j
    return lines


def _strip_one_trailing_eol(text):
    """Remove at most one trailing CRLF, LF, or CR from text."""
    if text.endswith("\r\n"):
        return text[:-2]
    if text.endswith("\n") or text.endswith("\r"):
        return text[:-1]
    return text


def _parse_message_text(text, factory, policy, headersonly=False, default_type=None):
    """Recursively parse an RFC 2822 / MIME message string."""
    msg = _make_message(factory, policy)
    if default_type is not None:
        msg.set_default_type(default_type)

    lines = _split_lines_keepends(text)
    idx = 0
    n = len(lines)

    # Optional Unix From_ envelope line
    if idx < n and lines[idx].startswith("From "):
        msg.set_unixfrom(lines[idx].rstrip("\r\n"))
        idx += 1

    # Parse header block
    cur_header = []

    def _flush_header():
        if not cur_header:
            return
        first = cur_header[0]
        if ":" in first:
            name, value = policy.header_source_parse(cur_header)
            msg.set_raw(name, value)
        cur_header.clear()

    while idx < n:
        line = lines[idx]
        if _EMPTY_LINE_RE.match(line):
            _flush_header()
            idx += 1
            break
        if line[0] in " \t":
            if cur_header:
                cur_header.append(line)
                idx += 1
                continue
            # Leading whitespace with no preceding header: treat as body start
            break
        colon = line.find(":")
        if colon > 0 and not any(ch in " \t\r\n" for ch in line[:colon]):
            _flush_header()
            cur_header.append(line)
            idx += 1
        else:
            # Not a valid header line — end of header section, line belongs to body
            _flush_header()
            break
    else:
        _flush_header()

    body = "".join(lines[idx:])

    if headersonly:
        msg.set_payload(body)
        return msg

    maintype = msg.get_content_maintype()
    subtype = msg.get_content_subtype()

    if maintype == "multipart":
        boundary = msg.get_boundary()
        if boundary is None:
            msg.set_payload(body)
            return msg
        _parse_multipart_body(msg, body, boundary, factory, policy, subtype)
        return msg

    if maintype == "message" and subtype in ("rfc822", "news", "global"):
        submsg = _parse_message_text(body, factory, policy, headersonly=False)
        msg.set_payload([submsg])
        return msg

    msg.set_payload(body)
    return msg


def _parse_multipart_body(msg, body, boundary, factory, policy, subtype):
    """Split a multipart body on RFC 2046 boundary lines and parse each subpart."""
    boundary_re = re.compile(
        r"^--" + re.escape(boundary) + r"(?P<end>--)?[ \t]*(\r\n|\r|\n|$)"
    )
    body_lines = _split_lines_keepends(body)

    preamble_lines = []
    parts_chunks = []
    epilogue_lines = None
    current_part = None
    seen_start = False

    for line in body_lines:
        if epilogue_lines is not None:
            epilogue_lines.append(line)
            continue
        mo = boundary_re.match(line)
        if mo:
            is_end = mo.group("end") == "--"
            if not seen_start:
                if is_end:
                    # Closing boundary before any start boundary
                    seen_start = True
                    epilogue_lines = []
                    continue
                seen_start = True
                current_part = []
            else:
                if current_part is not None:
                    parts_chunks.append(current_part)
                    current_part = None
                if is_end:
                    epilogue_lines = []
                else:
                    current_part = []
        else:
            if not seen_start:
                preamble_lines.append(line)
            elif current_part is not None:
                current_part.append(line)

    if current_part is not None:
        parts_chunks.append(current_part)

    if not seen_start:
        # No boundary found at all; keep empty multipart payload or raw body
        msg.set_payload([])
        if body:
            msg.preamble = body
        return

    if preamble_lines:
        msg.preamble = _strip_one_trailing_eol("".join(preamble_lines))
    else:
        msg.preamble = None

    if epilogue_lines is not None:
        msg.epilogue = "".join(epilogue_lines)
    else:
        msg.epilogue = None

    child_default_type = "message/rfc822" if subtype == "digest" else None
    subparts = []
    for chunk in parts_chunks:
        raw_part = _strip_one_trailing_eol("".join(chunk))
        submsg = _parse_message_text(
            raw_part,
            factory,
            policy,
            headersonly=False,
            default_type=child_default_type,
        )
        subparts.append(submsg)
    msg.set_payload(subparts)


class FeedParser:
    """Incremental string feed parser for RFC 2822 / MIME messages."""

    def __init__(self, _factory=None, *, policy=compat32):
        self.policy = policy
        self._factory = _factory
        self._headersonly = False
        self._buf = []

    def _set_headersonly(self):
        self._headersonly = True

    def feed(self, data):
        self._buf.append(data)

    def close(self):
        text = "".join(self._buf)
        self._buf.clear()
        return _parse_message_text(
            text, self._factory, self.policy, headersonly=self._headersonly
        )


class BytesFeedParser(FeedParser):
    """Incremental bytes feed parser for RFC 2822 / MIME messages."""

    def feed(self, data):
        super().feed(data.decode("ascii", "surrogateescape"))


class Parser:
    """Parser of RFC 2822 and MIME email messages from strings or text files."""

    def __init__(self, _class=None, *, policy=compat32):
        self._class = _class
        self.policy = policy

    def parse(self, fp, headersonly=False):
        feedparser = FeedParser(self._class, policy=self.policy)
        if headersonly:
            feedparser._set_headersonly()
        while True:
            data = fp.read(8192)
            if not data:
                break
            feedparser.feed(data)
        return feedparser.close()

    def parsestr(self, text, headersonly=False):
        return self.parse(StringIO(text), headersonly=headersonly)


class HeaderParser(Parser):
    """Parser that only parses message headers, leaving the body unparsed."""

    def parse(self, fp, headersonly=True):
        return Parser.parse(self, fp, True)

    def parsestr(self, text, headersonly=True):
        return Parser.parsestr(self, text, True)


class BytesParser:
    """Parser of binary RFC 2822 and MIME email messages."""

    def __init__(self, *args, **kw):
        self.parser = Parser(*args, **kw)

    def parse(self, fp, headersonly=False):
        chunks = []
        while True:
            data = fp.read(8192)
            if not data:
                break
            chunks.append(data)
        raw = b"".join(chunks)
        return self.parsebytes(raw, headersonly=headersonly)

    def parsebytes(self, text, headersonly=False):
        if isinstance(text, memoryview):
            text = text.tobytes()
        elif isinstance(text, bytearray):
            text = bytes(text)
        decoded = text.decode("ascii", errors="surrogateescape")
        return self.parser.parsestr(decoded, headersonly=headersonly)


class BytesHeaderParser(BytesParser):
    """Binary parser that only parses message headers."""

    def parse(self, fp, headersonly=True):
        return BytesParser.parse(self, fp, headersonly=True)

    def parsebytes(self, text, headersonly=True):
        return BytesParser.parsebytes(self, text, headersonly=True)
