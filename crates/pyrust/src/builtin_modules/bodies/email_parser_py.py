"""A parser of RFC 2822 and MIME email messages (issue #3211)."""

from io import StringIO

from email.feedparser import BytesFeedParser, FeedParser
from email.policy import compat32

__all__ = [
    "Parser",
    "HeaderParser",
    "BytesParser",
    "BytesHeaderParser",
    "FeedParser",
    "BytesFeedParser",
]


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
