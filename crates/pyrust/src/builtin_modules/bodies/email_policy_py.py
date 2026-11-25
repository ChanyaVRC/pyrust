"""Policy framework for the email package (issue #3211)."""

import abc
import binascii
import re
import sys

from email import header as _header
from email.utils import (
    _CHARSET_ALIASES,
    _UNKNOWN8BIT,
    _has_surrogates,
    _quopri_body_encode,
    _quopri_header_encode,
    _b64_header_encode,
    _quopri_header_length,
    _b64_header_length,
)

__all__ = [
    "Compat32",
    "compat32",
    "Policy",
    "EmailPolicy",
    "default",
    "strict",
    "SMTP",
    "HTTP",
]

linesep_splitter = re.compile(r"\n|\r")

# Headers with max_count == 1 in RFC 5322 / MIME (matching CPython's headerregistry)
_SINGLE_OCCURRENCE_HEADERS = frozenset(
    {
        "date",
        "from",
        "sender",
        "reply-to",
        "to",
        "cc",
        "bcc",
        "message-id",
        "subject",
        "orig-date",
        "mime-version",
        "content-type",
        "content-transfer-encoding",
        "content-disposition",
        "content-id",
    }
)


class _PolicyBase:
    """Immutable policy base class supporting clone() and '+' composition."""

    def __init__(self, **kw):
        for name, value in kw.items():
            if hasattr(self, name):
                object.__setattr__(self, name, value)
            else:
                raise TypeError(
                    "{!r} is an invalid keyword argument for {}".format(
                        name, self.__class__.__name__
                    )
                )

    def __repr__(self):
        args = [
            "{}={!r}".format(name, value)
            for name, value in self.__dict__.items()
        ]
        return "{}({})".format(self.__class__.__name__, ", ".join(args))

    def clone(self, **kw):
        newpolicy = self.__class__.__new__(self.__class__)
        for attr, value in self.__dict__.items():
            object.__setattr__(newpolicy, attr, value)
        for attr, value in kw.items():
            if not hasattr(self, attr):
                raise TypeError(
                    "{!r} is an invalid keyword argument for {}".format(
                        attr, self.__class__.__name__
                    )
                )
            object.__setattr__(newpolicy, attr, value)
        return newpolicy

    def __setattr__(self, name, value):
        if hasattr(self, name):
            msg = "{!r} object attribute {!r} is read-only"
        else:
            msg = "{!r} object has no attribute {!r}"
        raise AttributeError(msg.format(self.__class__.__name__, name))

    def __add__(self, other):
        return self.clone(**other.__dict__)


class Policy(_PolicyBase, metaclass=abc.ABCMeta):
    """Abstract base class for email policies."""

    raise_on_defect = False
    linesep = "\n"
    cte_type = "8bit"
    max_line_length = 78
    mangle_from_ = False
    message_factory = None

    def handle_defect(self, obj, defect):
        if self.raise_on_defect:
            raise defect
        self.register_defect(obj, defect)

    def register_defect(self, obj, defect):
        obj.defects.append(defect)

    def header_max_count(self, name):
        return None

    @abc.abstractmethod
    def header_source_parse(self, sourcelines):
        raise NotImplementedError

    @abc.abstractmethod
    def header_store_parse(self, name, value):
        raise NotImplementedError

    @abc.abstractmethod
    def header_fetch_parse(self, name, value):
        raise NotImplementedError

    @abc.abstractmethod
    def fold(self, name, value):
        raise NotImplementedError

    @abc.abstractmethod
    def fold_binary(self, name, value):
        raise NotImplementedError


class Compat32(Policy):
    """Backward-compatibility Policy replicating email 5.1 behavior."""

    mangle_from_ = True

    def _sanitize_header(self, name, value):
        if not isinstance(value, str):
            return value
        if _has_surrogates(value):
            return _header.Header(
                value, charset=_UNKNOWN8BIT, header_name=name
            )
        return value

    def header_source_parse(self, sourcelines):
        name, value = sourcelines[0].split(":", 1)
        value = value.lstrip(" \t") + "".join(sourcelines[1:])
        return (name, value.rstrip("\r\n"))

    def header_store_parse(self, name, value):
        return (name, value)

    def header_fetch_parse(self, name, value):
        return self._sanitize_header(name, value)

    def fold(self, name, value):
        return self._fold(name, value, sanitize=True)

    def fold_binary(self, name, value):
        folded = self._fold(name, value, sanitize=self.cte_type == "7bit")
        return folded.encode("ascii", "surrogateescape")

    def _fold(self, name, value, sanitize):
        parts = ["%s: " % name]
        if isinstance(value, str):
            if _has_surrogates(value):
                if sanitize:
                    h = _header.Header(
                        value, charset=_UNKNOWN8BIT, header_name=name
                    )
                else:
                    parts.append(value)
                    h = None
            else:
                h = _header.Header(value, header_name=name)
        else:
            h = value
        if h is not None:
            maxlinelen = 0
            if self.max_line_length is not None:
                maxlinelen = self.max_line_length
            parts.append(
                h.encode(linesep=self.linesep, maxlinelen=maxlinelen)
            )
        parts.append(self.linesep)
        return "".join(parts)


compat32 = Compat32()


# ── ContentManager (raw_data_manager) for EmailPolicy / EmailMessage ─────────

from email.contentmanager import ContentManager, raw_data_manager


# ── EmailPolicy ──────────────────────────────────────────────────────────────


def _decode_rfc2047_value(value):
    """Decode RFC 2047 encoded words in a header string for EmailPolicy."""
    if "=?" not in value:
        return value
    try:
        parts = _header.decode_header(value)
    except Exception:
        return value
    out = []
    for chunk, charset in parts:
        if isinstance(chunk, bytes):
            enc = charset or "ascii"
            if enc == _UNKNOWN8BIT:
                out.append(chunk.decode("ascii", "replace"))
            else:
                try:
                    out.append(chunk.decode(enc, "replace"))
                except LookupError:
                    out.append(chunk.decode("ascii", "replace"))
        else:
            out.append(chunk)
    return "".join(out)


def _encode_non_ascii_header_word(word):
    """Encode a single non-ASCII word using RFC 2047 UTF-8 (shortest of Q/B)."""
    b = word.encode("utf-8")
    len_q = _quopri_header_length(b)
    len_b = _b64_header_length(b)
    if len_b < len_q:
        return _b64_header_encode(b, "utf-8")
    return _quopri_header_encode(b, "utf-8")


def _fold_email_header(name, value, policy):
    """Fold a header according to EmailPolicy settings."""
    if not isinstance(value, str):
        value = str(value)
    linesep = policy.linesep
    maxlen = policy.max_line_length if policy.max_line_length else sys.maxsize

    # Unfold any existing line breaks
    unfolded = "".join(linesep_splitter.split(value))

    if not policy.utf8 and not unfolded.isascii():
        # Encode non-ASCII header text with RFC 2047
        h = _header.Header(
            unfolded,
            charset="utf-8",
            maxlinelen=0 if policy.max_line_length is None else policy.max_line_length,
            header_name=name,
        )
        encoded = h.encode(linesep=linesep)
        return name + ": " + encoded + linesep

    # ASCII or UTF-8 passthrough with simple folding when exceeding maxlen
    full_first = name + ": " + unfolded
    if len(full_first) <= maxlen:
        return full_first + linesep

    h = _header.Header(
        unfolded,
        charset="utf-8" if policy.utf8 else "us-ascii",
        maxlinelen=0 if policy.max_line_length is None else policy.max_line_length,
        header_name=name,
    )
    if policy.utf8 and not unfolded.isascii():
        # Keep UTF-8 unencoded, wrap at whitespace boundaries
        words = re.split(r"([ \t]+)", unfolded)
        lines = []
        current = name + ": "
        for tok in words:
            if not tok:
                continue
            if len(current) + len(tok) > maxlen and current.strip() and current != name + ": ":
                lines.append(current.rstrip(" \t"))
                current = " " + tok.lstrip(" \t")
            else:
                current += tok
        lines.append(current)
        return linesep.join(lines) + linesep

    encoded = h.encode(linesep=linesep)
    return name + ": " + encoded + linesep


class EmailPolicy(Policy):
    """Modern email policy (EmailMessage default, SMTP, SMTPUTF8, HTTP, strict)."""

    utf8 = False
    refold_source = "long"
    content_manager = raw_data_manager

    @property
    def message_factory(self):
        if "message_factory" in self.__dict__:
            return self.__dict__["message_factory"]
        from email.message import EmailMessage

        return EmailMessage

    def __init__(self, **kw):
        super().__init__(**kw)

    def header_max_count(self, name):
        if name.lower() in _SINGLE_OCCURRENCE_HEADERS:
            return 1
        return None

    def header_source_parse(self, sourcelines):
        name, value = sourcelines[0].split(":", 1)
        value = value.lstrip(" \t") + "".join(sourcelines[1:])
        return (name, value.rstrip("\r\n"))

    def header_store_parse(self, name, value):
        if isinstance(value, str) and len(value.splitlines()) > 1:
            raise ValueError(
                "Header values may not contain linefeed "
                "or carriage return characters"
            )
        return (name, value)

    def header_fetch_parse(self, name, value):
        if not isinstance(value, str):
            return value
        if _has_surrogates(value):
            value = value.encode("ascii", "surrogateescape").decode(
                "utf-8", "replace"
            )
        unfolded = "".join(linesep_splitter.split(value))
        return _decode_rfc2047_value(unfolded)

    def fold(self, name, value):
        return _fold_email_header(name, value, self)

    def fold_binary(self, name, value):
        folded = _fold_email_header(name, value, self)
        charset = "utf-8" if self.utf8 else "ascii"
        return folded.encode(charset, "surrogateescape")


default = EmailPolicy()
strict = default.clone(raise_on_defect=True)
SMTP = default.clone(linesep="\r\n")
HTTP = default.clone(linesep="\r\n", max_line_length=None)
SMTPUTF8 = SMTP.clone(utf8=True)
