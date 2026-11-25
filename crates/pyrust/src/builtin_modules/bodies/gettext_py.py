"""Internationalization and localization support (CPython 3.12 parity, issue #3226)."""

import builtins
import copy
import errno
import locale
import os
import re
import struct
import sys

__all__ = [
    "NullTranslations",
    "GNUTranslations",
    "Catalog",
    "bindtextdomain",
    "find",
    "translation",
    "install",
    "textdomain",
    "dgettext",
    "dngettext",
    "gettext",
    "ngettext",
    "pgettext",
    "dpgettext",
    "npgettext",
    "dnpgettext",
]

_default_localedir = os.path.join(sys.base_prefix, "share", "locale")

_missing = object()


# ---------------------------------------------------------------------------
# Safe GNU gettext plural-forms expression compiler (c2py)
# ---------------------------------------------------------------------------

_TOKEN_RE = re.compile(
    r"""
        \s*(?:
            (?P<op>\|\||&&|==|!=|<=|>=|[?:()!+\-*/%<>]) |
            (?P<num>[0-9]+) |
            (?P<ident>[A-Za-z_][A-Za-z0-9_]*) |
            (?P<bad>\S)
        )
    """,
    re.VERBOSE,
)

_MAX_PLURAL_DEPTH = 64


def _tokenize_plural(plural):
    tokens = []
    pos = 0
    length = len(plural)
    while pos < length:
        m = _TOKEN_RE.match(plural, pos)
        if m is None:
            break
        pos = m.end()
        if m.group("op") is not None:
            tokens.append(("OP", m.group("op")))
        elif m.group("num") is not None:
            tokens.append(("NUM", int(m.group("num"))))
        elif m.group("ident") is not None:
            ident = m.group("ident")
            if ident != "n":
                raise ValueError(f"invalid token in plural form: {ident}")
            tokens.append(("VAR", "n"))
        else:
            bad = m.group("bad")
            raise ValueError(f"invalid token in plural form: {bad}")
    tokens.append(("EOF", ""))
    return tokens


class _PluralParser:
    """Recursive-descent parser compiling GNU C plural expressions into closures."""

    def __init__(self, tokens):
        self._tokens = tokens
        self._pos = 0
        self._depth = 0

    def _peek(self):
        return self._tokens[self._pos]

    def _advance(self):
        tok = self._tokens[self._pos]
        self._pos += 1
        return tok

    def _enter(self):
        self._depth += 1
        if self._depth > _MAX_PLURAL_DEPTH:
            raise ValueError("plural form expression is too complex")

    def _leave(self):
        self._depth -= 1

    def parse(self):
        fn = self._parse_ternary()
        kind, val = self._peek()
        if kind != "EOF":
            if val == ")":
                raise ValueError("unbalanced parenthesis in plural form")
            raise ValueError(f"unexpected token in plural form: {val}")
        return fn

    def _parse_ternary(self):
        self._enter()
        try:
            cond = self._parse_or()
            if self._peek() == ("OP", "?"):
                self._advance()
                true_fn = self._parse_ternary()
                kind, val = self._peek()
                if kind == "EOF":
                    raise ValueError("unexpected end of plural form")
                if (kind, val) != ("OP", ":"):
                    raise ValueError(f"unexpected token in plural form: {val}")
                self._advance()
                false_fn = self._parse_ternary()
                return lambda n, c=cond, t=true_fn, f=false_fn: (
                    t(n) if c(n) else f(n)
                )
            return cond
        finally:
            self._leave()

    def _parse_or(self):
        left = self._parse_and()
        while self._peek() == ("OP", "||"):
            self._advance()
            right = self._parse_and()
            left = lambda n, l=left, r=right: int(bool(l(n)) or bool(r(n)))
        return left

    def _parse_and(self):
        left = self._parse_eq()
        while self._peek() == ("OP", "&&"):
            self._advance()
            right = self._parse_eq()
            left = lambda n, l=left, r=right: int(bool(l(n)) and bool(r(n)))
        return left

    def _parse_eq(self):
        left = self._parse_rel()
        while self._peek()[0] == "OP" and self._peek()[1] in ("==", "!="):
            op = self._advance()[1]
            right = self._parse_rel()
            if op == "==":
                left = lambda n, l=left, r=right: int(l(n) == r(n))
            else:
                left = lambda n, l=left, r=right: int(l(n) != r(n))
        return left

    def _parse_rel(self):
        left = self._parse_add()
        while self._peek()[0] == "OP" and self._peek()[1] in ("<", "<=", ">", ">="):
            op = self._advance()[1]
            right = self._parse_add()
            if op == "<":
                left = lambda n, l=left, r=right: int(l(n) < r(n))
            elif op == "<=":
                left = lambda n, l=left, r=right: int(l(n) <= r(n))
            elif op == ">":
                left = lambda n, l=left, r=right: int(l(n) > r(n))
            else:
                left = lambda n, l=left, r=right: int(l(n) >= r(n))
        return left

    def _parse_add(self):
        left = self._parse_mul()
        while self._peek()[0] == "OP" and self._peek()[1] in ("+", "-"):
            op = self._advance()[1]
            right = self._parse_mul()
            if op == "+":
                left = lambda n, l=left, r=right: l(n) + r(n)
            else:
                left = lambda n, l=left, r=right: l(n) - r(n)
        return left

    def _parse_mul(self):
        left = self._parse_unary()
        while self._peek()[0] == "OP" and self._peek()[1] in ("*", "/", "%"):
            op = self._advance()[1]
            right = self._parse_unary()
            if op == "*":
                left = lambda n, l=left, r=right: l(n) * r(n)
            elif op == "/":
                left = lambda n, l=left, r=right: l(n) // r(n)
            else:
                left = lambda n, l=left, r=right: l(n) % r(n)
        return left

    def _parse_unary(self):
        if self._peek() == ("OP", "!"):
            self._advance()
            operand = self._parse_unary()
            return lambda n, o=operand: int(not o(n))
        return self._parse_primary()

    def _parse_primary(self):
        kind, val = self._peek()
        if kind == "NUM":
            self._advance()
            return lambda n, v=val: v
        if kind == "VAR":
            self._advance()
            return lambda n: n
        if (kind, val) == ("OP", "("):
            self._advance()
            inner = self._parse_ternary()
            next_kind, next_val = self._peek()
            if (next_kind, next_val) != ("OP", ")"):
                if next_kind == "EOF":
                    raise ValueError("unbalanced parenthesis in plural form")
                raise ValueError(f"unexpected token in plural form: {next_val}")
            self._advance()
            return inner
        if kind == "EOF":
            raise ValueError("unexpected end of plural form")
        raise ValueError(f"unexpected token in plural form: {val}")


def c2py(plural):
    """Compile a C-style GNU gettext plural expression into a Python function."""
    if not isinstance(plural, str):
        raise TypeError(
            f"plural must be a str, not {type(plural).__name__}"
        )
    tokens = _tokenize_plural(plural)
    if tokens[0][0] == "EOF":
        raise ValueError("unexpected end of plural form")
    compiled = _PluralParser(tokens).parse()

    def _plural_func(n):
        return int(compiled(n))

    return _plural_func


# ---------------------------------------------------------------------------
# Language expansion (matching CPython 3.12 Lib/gettext.py)
# ---------------------------------------------------------------------------

def _expand_lang(loc):
    loc = locale.normalize(loc)
    COMPONENT_CODESET = 1 << 0
    COMPONENT_TERRITORY = 1 << 1
    COMPONENT_MODIFIER = 1 << 2
    mask = 0
    pos = loc.find("@")
    if pos >= 0:
        modifier = loc[pos:]
        loc = loc[:pos]
        mask |= COMPONENT_MODIFIER
    else:
        modifier = ""
    pos = loc.find(".")
    if pos >= 0:
        codeset = loc[pos:]
        loc = loc[:pos]
        mask |= COMPONENT_CODESET
    else:
        codeset = ""
    pos = loc.find("_")
    if pos >= 0:
        territory = loc[pos:]
        loc = loc[:pos]
        mask |= COMPONENT_TERRITORY
    else:
        territory = ""
    language = loc
    ret = []
    for i in range(mask + 1):
        if not (i & ~mask):
            val = language
            if i & COMPONENT_TERRITORY:
                val += territory
            if i & COMPONENT_CODESET:
                val += codeset
            if i & COMPONENT_MODIFIER:
                val += modifier
            ret.append(val)
    ret.reverse()
    return ret


# ---------------------------------------------------------------------------
# Translation classes
# ---------------------------------------------------------------------------

class NullTranslations:
    """Base translation class performing pass-through (untranslated) lookups."""

    def __init__(self, fp=None):
        self._info = {}
        self._charset = None
        self._output_charset = None
        self._fallback = None
        if fp is not None:
            self._parse(fp)

    def _parse(self, fp):
        pass

    def add_fallback(self, fallback):
        if self._fallback:
            self._fallback.add_fallback(fallback)
        else:
            self._fallback = fallback

    def set_fallback(self, fallback):
        self._fallback = fallback

    def gettext(self, message):
        if self._fallback:
            return self._fallback.gettext(message)
        return message

    def ngettext(self, msgid1, msgid2, n):
        if self._fallback:
            return self._fallback.ngettext(msgid1, msgid2, n)
        if n == 1:
            return msgid1
        return msgid2

    def pgettext(self, context, message):
        if self._fallback:
            return self._fallback.pgettext(context, message)
        return message

    def npgettext(self, context, msgid1, msgid2, n):
        if self._fallback:
            return self._fallback.npgettext(context, msgid1, msgid2, n)
        if n == 1:
            return msgid1
        return msgid2

    def info(self):
        return self._info

    def charset(self):
        return self._charset

    def output_charset(self):
        return self._output_charset

    def set_output_charset(self, charset):
        self._output_charset = charset

    def install(self, names=None):
        builtins.__dict__["_"] = self.gettext
        if names is not None:
            allowed = {"gettext", "ngettext", "npgettext", "pgettext"}
            for name in allowed & set(names):
                builtins.__dict__[name] = getattr(self, name)


class GNUTranslations(NullTranslations):
    """GNU gettext binary `.mo` message catalog reader."""

    LE_MAGIC = 0x950412DE
    BE_MAGIC = 0xDE120495

    CONTEXT = "%s\x04%s"

    # Acceptable .mo major versions
    VERSIONS = (0, 1)

    def _get_versions(self, version):
        return (version >> 16, version & 0xFFFF)

    def _parse(self, fp):
        """Parse a GNU `.mo` file from binary file-like object `fp`."""
        unpack = struct.unpack
        filename = getattr(fp, "name", "")
        self._catalog = catalog = {}
        self.plural = lambda n: int(n != 1)  # Germanic plural by default
        buf = fp.read()
        buflen = len(buf)
        magic = unpack("<I", buf[:4])[0]
        if magic == self.LE_MAGIC:
            version, msgcount, masteridx, transidx = unpack("<4I", buf[4:20])
            ii = "<II"
        elif magic == self.BE_MAGIC:
            version, msgcount, masteridx, transidx = unpack(">4I", buf[4:20])
            ii = ">II"
        else:
            raise OSError(0, "Bad magic number", filename)

        major_version, minor_version = self._get_versions(version)
        if major_version not in self.VERSIONS:
            raise OSError(0, f"Bad version number {major_version}", filename)

        for _ in range(msgcount):
            if masteridx + 8 > buflen or transidx + 8 > buflen:
                raise OSError(0, "File is corrupt", filename)
            mlen, moff = unpack(ii, buf[masteridx : masteridx + 8])
            mend = moff + mlen
            tlen, toff = unpack(ii, buf[transidx : transidx + 8])
            tend = toff + tlen
            if mend < buflen and tend < buflen:
                msg = buf[moff:mend]
                tmsg = buf[toff:tend]
            elif mend <= buflen and tend <= buflen:
                msg = buf[moff:mend]
                tmsg = buf[toff:tend]
            else:
                raise OSError(0, "File is corrupt", filename)

            # Empty msgid carries RFC-822-style catalog metadata header.
            if mlen == 0:
                lastk = None
                for b_item in tmsg.split(b"\n"):
                    item = b_item.decode().strip()
                    if not item:
                        continue
                    # Skip #-#-#-#-#  ...  #-#-#-#-# merge comments
                    if item.startswith("#-#-#-#-#") and item.endswith("#-#-#-#-#"):
                        continue
                    k = v = None
                    if ":" in item:
                        k, v = item.split(":", 1)
                        k = k.strip().lower()
                        v = v.strip()
                        self._info[k] = v
                        lastk = k
                    elif lastk:
                        self._info[lastk] += "\n" + item
                    if k == "content-type":
                        parts = v.split("charset=")
                        if len(parts) > 1:
                            self._charset = parts[1].split(";")[0].strip()
                    elif k == "plural-forms":
                        v_parts = v.split(";")
                        plural_expr = None
                        for part in v_parts:
                            part = part.strip()
                            if part.startswith("plural="):
                                plural_expr = part[len("plural=") :].strip()
                                break
                        if plural_expr is not None:
                            self.plural = c2py(plural_expr)

            charset = self._charset or "ascii"
            if b"\x00" in msg:
                # Plural forms: msgid1\x00msgid2 -> msgstr0\x00msgstr1\x00...
                msgid1, msgid2 = msg.split(b"\x00", 1)
                tmsgs = tmsg.split(b"\x00")
                msgid1_s = str(msgid1, charset)
                for idx, x in enumerate(tmsgs):
                    catalog[(msgid1_s, idx)] = str(x, charset)
            else:
                catalog[str(msg, charset)] = str(tmsg, charset)

            masteridx += 8
            transidx += 8

    def gettext(self, message):
        tmsg = self._catalog.get(message, _missing)
        if tmsg is _missing:
            if self._fallback:
                return self._fallback.gettext(message)
            return message
        return tmsg

    def ngettext(self, msgid1, msgid2, n):
        try:
            tmsg = self._catalog[(msgid1, self.plural(n))]
        except KeyError:
            if self._fallback:
                return self._fallback.ngettext(msgid1, msgid2, n)
            if n == 1:
                tmsg = msgid1
            else:
                tmsg = msgid2
        return tmsg

    def pgettext(self, context, message):
        ctxt_msg_id = self.CONTEXT % (context, message)
        tmsg = self._catalog.get(ctxt_msg_id, _missing)
        if tmsg is _missing:
            if self._fallback:
                return self._fallback.pgettext(context, message)
            return message
        return tmsg

    def npgettext(self, context, msgid1, msgid2, n):
        ctxt_msg_id = self.CONTEXT % (context, msgid1)
        try:
            tmsg = self._catalog[(ctxt_msg_id, self.plural(n))]
        except KeyError:
            if self._fallback:
                return self._fallback.npgettext(context, msgid1, msgid2, n)
            if n == 1:
                tmsg = msgid1
            else:
                tmsg = msgid2
        return tmsg


# ---------------------------------------------------------------------------
# Catalog discovery and global domain API
# ---------------------------------------------------------------------------

def find(domain, localedir=None, languages=None, all=False):
    """Locate `.mo` message catalog file(s) for `domain`."""
    if localedir is None:
        localedir = _default_localedir
    if languages is None:
        languages = []
        for envar in ("LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"):
            val = os.environ.get(envar)
            if val:
                languages = val.split(":")
                break
        if "C" not in languages:
            languages.append("C")

    nelangs = []
    for lang in languages:
        for nelang in _expand_lang(lang):
            if nelang not in nelangs:
                nelangs.append(nelang)

    result = [] if all else None
    for lang in nelangs:
        if lang == "C":
            break
        mofile = os.path.join(localedir, lang, "LC_MESSAGES", f"{domain}.mo")
        if os.path.exists(mofile):
            if all:
                result.append(mofile)
            else:
                return mofile
    return result


_translations = {}


def translation(domain, localedir=None, languages=None, class_=None, fallback=False):
    """Return a Translations instance for `domain`."""
    if class_ is None:
        class_ = GNUTranslations
    mofiles = find(domain, localedir, languages, all=True)
    if not mofiles:
        if fallback:
            return NullTranslations()
        raise FileNotFoundError(
            errno.ENOENT, "No translation file found for domain", domain
        )

    result = None
    for mofile in mofiles:
        key = (class_, os.path.abspath(mofile))
        t = _translations.get(key)
        if t is None:
            with open(mofile, "rb") as fp:
                t = _translations.setdefault(key, class_(fp))
        t = copy.copy(t)
        if result is None:
            result = t
        else:
            result.add_fallback(t)
    return result


Catalog = translation


def install(domain, localedir=None, *, names=None):
    """Install `domain` translation functions into `builtins`."""
    t = translation(domain, localedir, fallback=True)
    t.install(names)


_localedirs = {}
_current_domain = "messages"


def textdomain(domain=None):
    """Query or change the current global domain."""
    global _current_domain
    if domain is not None:
        _current_domain = domain
    return _current_domain


def bindtextdomain(domain, localedir=None):
    """Query or bind `domain` to `localedir`."""
    if localedir is not None:
        _localedirs[domain] = localedir
    return _localedirs.get(domain, _default_localedir)


def dgettext(domain, message):
    try:
        t = translation(domain, _localedirs.get(domain, None))
    except OSError:
        return message
    return t.gettext(message)


def dngettext(domain, msgid1, msgid2, n):
    try:
        t = translation(domain, _localedirs.get(domain, None))
    except OSError:
        if n == 1:
            return msgid1
        return msgid2
    return t.ngettext(msgid1, msgid2, n)


def dpgettext(domain, context, message):
    try:
        t = translation(domain, _localedirs.get(domain, None))
    except OSError:
        return message
    return t.pgettext(context, message)


def dnpgettext(domain, context, msgid1, msgid2, n):
    try:
        t = translation(domain, _localedirs.get(domain, None))
    except OSError:
        if n == 1:
            return msgid1
        return msgid2
    return t.npgettext(context, msgid1, msgid2, n)


def gettext(message):
    return dgettext(_current_domain, message)


def ngettext(msgid1, msgid2, n):
    return dngettext(_current_domain, msgid1, msgid2, n)


def pgettext(context, message):
    return dpgettext(_current_domain, context, message)


def npgettext(context, msgid1, msgid2, n):
    return dnpgettext(_current_domain, context, msgid1, msgid2, n)
