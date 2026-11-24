"""
JSON encoder/decoder — pyrust port of CPython 3.12's ``json`` package.

Provides ``dumps`` / ``dump`` / ``loads`` / ``load``, ``JSONEncoder`` /
``JSONDecoder``, ``detect_encoding``, and ``JSONDecodeError``.  Behaviour
(output formatting, type mapping, escape handling, hooks, circular reference
detection, encoding detection, and error wording) targets CPython 3.12.

This source is exec'd once into a throwaway namespace at first ``import json``
and the public names are copied onto the module by
``json.rs::inject_python_members`` (mirrors ``operator`` / ``string``).
"""

# --------------------------------------------------------------------------- #
# Exceptions
# --------------------------------------------------------------------------- #


class JSONDecodeError(ValueError):
    """Subclass of ValueError with the following additional properties:

    msg: The unformatted error message
    doc: The JSON document being parsed
    pos: The start index of doc where parsing failed
    lineno: The line corresponding to pos
    colno: The column corresponding to pos
    """

    __module__ = "json.decoder"

    def __init__(self, msg, doc, pos):
        lineno = doc.count('\n', 0, pos) + 1
        colno = pos - doc.rfind('\n', 0, pos)
        errmsg = '%s: line %d column %d (char %d)' % (msg, lineno, colno, pos)
        ValueError.__init__(self, errmsg)
        self.msg = msg
        self.doc = doc
        self.pos = pos
        self.lineno = lineno
        self.colno = colno

    def __reduce__(self):
        return self.__class__, (self.msg, self.doc, self.pos)


# --------------------------------------------------------------------------- #
# Encoding detection (CPython 3.12 json.detect_encoding)
# --------------------------------------------------------------------------- #

_BOM_UTF8 = b'\xef\xbb\xbf'
_BOM_UTF16_LE = b'\xff\xfe'
_BOM_UTF16_BE = b'\xfe\xff'
_BOM_UTF32_LE = b'\xff\xfe\x00\x00'
_BOM_UTF32_BE = b'\x00\x00\xfe\xff'


def detect_encoding(b):
    bstartswith = b.startswith
    if bstartswith((_BOM_UTF32_BE, _BOM_UTF32_LE)):
        return 'utf-32'
    if bstartswith((_BOM_UTF16_BE, _BOM_UTF16_LE)):
        return 'utf-16'
    if bstartswith(_BOM_UTF8):
        return 'utf-8-sig'

    if len(b) >= 4:
        if not b[0]:
            # 00 00 -- -- -> utf-32-be
            # 00 XX -- -- -> utf-16-be
            return 'utf-16-be' if b[1] else 'utf-32-be'
        if not b[1]:
            # XX 00 00 00 -> utf-32-le
            # XX 00 XX -- -> utf-16-le
            return 'utf-16-le' if b[2] or b[3] else 'utf-32-le'
    elif len(b) == 2:
        if not b[0]:
            # 00 XX -> utf-16-be
            return 'utf-16-be'
        if not b[1]:
            # XX 00 -> utf-16-le
            return 'utf-16-le'
    return 'utf-8'


# --------------------------------------------------------------------------- #
# Encoder
# --------------------------------------------------------------------------- #

# Characters that must be escaped inside a JSON string, plus their escape.
_ESCAPE_MAP = {
    '"': '\\"',
    '\\': '\\\\',
    '\n': '\\n',
    '\r': '\\r',
    '\t': '\\t',
    '\b': '\\b',
    '\f': '\\f',
}

_INFINITY = float('inf')


def _encode_string(s, ensure_ascii):
    """Return the JSON-quoted form of the string *s*."""
    out = ['"']
    for ch in s:
        esc = _ESCAPE_MAP.get(ch)
        if esc is not None:
            out.append(esc)
        elif ch < '\x20':
            out.append('\\u%04x' % ord(ch))
        elif ensure_ascii and ch > '\x7f':
            cp = ord(ch)
            if cp > 0xffff:
                # Emit a UTF-16 surrogate pair.
                cp -= 0x10000
                hi = 0xd800 + (cp >> 10)
                lo = 0xdc00 + (cp & 0x3ff)
                out.append('\\u%04x\\u%04x' % (hi, lo))
            else:
                out.append('\\u%04x' % cp)
        else:
            out.append(ch)
    out.append('"')
    return ''.join(out)


class JSONEncoder:
    """Extensible JSON encoder for Python data structures."""

    __module__ = "json.encoder"

    item_separator = ', '
    key_separator = ': '

    def __init__(self, *, skipkeys=False, ensure_ascii=True,
                 check_circular=True, allow_nan=True, sort_keys=False,
                 indent=None, separators=None, default=None):
        self.skipkeys = skipkeys
        self.ensure_ascii = ensure_ascii
        self.check_circular = check_circular
        self.allow_nan = allow_nan
        self.sort_keys = sort_keys
        self.indent = indent
        if separators is not None:
            self.item_separator, self.key_separator = separators
        elif indent is not None:
            self.item_separator = ','
        if default is not None:
            self.default = default

    def default(self, o):
        """Implement this method in a subclass such that it returns a
        serializable object for ``o``, or calls the base implementation."""
        raise TypeError(
            'Object of type %s is not JSON serializable' % o.__class__.__name__)

    def encode(self, o):
        """Return a JSON string representation of a Python data structure."""
        if isinstance(o, str):
            return _encode_string(o, self.ensure_ascii)
        chunks = self.iterencode(o, _one_shot=True)
        if not isinstance(chunks, (list, tuple)):
            chunks = list(chunks)
        return ''.join(chunks)

    def iterencode(self, o, _one_shot=False):
        """Encode the given object and yield each string representation as
        available."""
        markers = {} if self.check_circular else None
        ensure_ascii = self.ensure_ascii
        allow_nan = self.allow_nan
        skipkeys = self.skipkeys
        sort_keys = self.sort_keys
        key_separator = self.key_separator
        item_separator = self.item_separator
        indent = self.indent
        if isinstance(indent, int):
            indent_str = ' ' * indent
        elif isinstance(indent, str):
            indent_str = indent
        else:
            indent_str = None

        def floatstr(val):
            if val != val:
                text = 'NaN'
            elif val == _INFINITY:
                text = 'Infinity'
            elif val == -_INFINITY:
                text = '-Infinity'
            else:
                return float.__repr__(val)
            if not allow_nan:
                raise ValueError(
                    'Out of range float values are not JSON compliant: '
                    + repr(val))
            return text

        def _iterencode_list(lst, current_indent_level):
            if not lst:
                yield '[]'
                return
            if markers is not None:
                markerid = id(lst)
                if markerid in markers:
                    raise ValueError('Circular reference detected')
                markers[markerid] = lst
            try:
                buf = '['
                if indent_str is not None:
                    current_indent_level += 1
                    newline_indent = '\n' + indent_str * current_indent_level
                    separator = item_separator + newline_indent
                    buf += newline_indent
                else:
                    newline_indent = None
                    separator = item_separator
                first = True
                for value in lst:
                    if first:
                        first = False
                    else:
                        buf = separator
                    if isinstance(value, str):
                        yield buf + _encode_string(value, ensure_ascii)
                    elif value is None:
                        yield buf + 'null'
                    elif value is True:
                        yield buf + 'true'
                    elif value is False:
                        yield buf + 'false'
                    elif isinstance(value, int):
                        yield buf + int.__repr__(value)
                    elif isinstance(value, float):
                        yield buf + floatstr(value)
                    else:
                        yield buf
                        if isinstance(value, (list, tuple)):
                            chunks = _iterencode_list(value, current_indent_level)
                        elif isinstance(value, dict):
                            chunks = _iterencode_dict(value, current_indent_level)
                        else:
                            chunks = _iterencode(value, current_indent_level)
                        yield from chunks
                if newline_indent is not None:
                    current_indent_level -= 1
                    yield '\n' + indent_str * current_indent_level
                yield ']'
            finally:
                if markers is not None:
                    del markers[markerid]

        def _iterencode_dict(dct, current_indent_level):
            if not dct:
                yield '{}'
                return
            if markers is not None:
                markerid = id(dct)
                if markerid in markers:
                    raise ValueError('Circular reference detected')
                markers[markerid] = dct
            try:
                yield '{'
                if indent_str is not None:
                    current_indent_level += 1
                    newline_indent = '\n' + indent_str * current_indent_level
                    item_sep = item_separator + newline_indent
                    yield newline_indent
                else:
                    newline_indent = None
                    item_sep = item_separator
                first = True
                if sort_keys:
                    items = sorted(dct.items(), key=lambda kv: kv[0])
                else:
                    items = dct.items()
                for key, value in items:
                    if isinstance(key, str):
                        pass
                    elif isinstance(key, float):
                        key = floatstr(key)
                    elif key is True:
                        key = 'true'
                    elif key is False:
                        key = 'false'
                    elif key is None:
                        key = 'null'
                    elif isinstance(key, int):
                        key = int.__repr__(key)
                    elif skipkeys:
                        continue
                    else:
                        raise TypeError(
                            'keys must be str, int, float, bool or None, not %s'
                            % key.__class__.__name__)
                    if first:
                        first = False
                    else:
                        yield item_sep
                    yield _encode_string(key, ensure_ascii)
                    yield key_separator
                    if isinstance(value, str):
                        yield _encode_string(value, ensure_ascii)
                    elif value is None:
                        yield 'null'
                    elif value is True:
                        yield 'true'
                    elif value is False:
                        yield 'false'
                    elif isinstance(value, int):
                        yield int.__repr__(value)
                    elif isinstance(value, float):
                        yield floatstr(value)
                    else:
                        if isinstance(value, (list, tuple)):
                            chunks = _iterencode_list(value, current_indent_level)
                        elif isinstance(value, dict):
                            chunks = _iterencode_dict(value, current_indent_level)
                        else:
                            chunks = _iterencode(value, current_indent_level)
                        yield from chunks
                if newline_indent is not None:
                    current_indent_level -= 1
                    yield '\n' + indent_str * current_indent_level
                yield '}'
            finally:
                if markers is not None:
                    del markers[markerid]

        def _iterencode(val, current_indent_level):
            if isinstance(val, str):
                yield _encode_string(val, ensure_ascii)
            elif val is None:
                yield 'null'
            elif val is True:
                yield 'true'
            elif val is False:
                yield 'false'
            elif isinstance(val, int):
                yield int.__repr__(val)
            elif isinstance(val, float):
                yield floatstr(val)
            elif isinstance(val, (list, tuple)):
                yield from _iterencode_list(val, current_indent_level)
            elif isinstance(val, dict):
                yield from _iterencode_dict(val, current_indent_level)
            else:
                if markers is not None:
                    markerid = id(val)
                    if markerid in markers:
                        raise ValueError('Circular reference detected')
                    markers[markerid] = val
                try:
                    val = self.default(val)
                    yield from _iterencode(val, current_indent_level)
                finally:
                    if markers is not None:
                        del markers[markerid]

        return _iterencode(o, 0)


_default_encoder = JSONEncoder(
    skipkeys=False,
    ensure_ascii=True,
    check_circular=True,
    allow_nan=True,
    indent=None,
    separators=None,
    default=None,
)


def dumps(obj, *, skipkeys=False, ensure_ascii=True, check_circular=True,
          allow_nan=True, cls=None, indent=None, separators=None,
          default=None, sort_keys=False, **kw):
    """Serialize *obj* to a JSON formatted ``str``."""
    if (not skipkeys and ensure_ascii and check_circular and allow_nan
            and cls is None and indent is None and separators is None
            and default is None and not sort_keys and not kw):
        return _default_encoder.encode(obj)
    if cls is None:
        cls = JSONEncoder
    return cls(
        skipkeys=skipkeys,
        ensure_ascii=ensure_ascii,
        check_circular=check_circular,
        allow_nan=allow_nan,
        indent=indent,
        separators=separators,
        default=default,
        sort_keys=sort_keys,
        **kw,
    ).encode(obj)


def dump(obj, fp, *, skipkeys=False, ensure_ascii=True, check_circular=True,
         allow_nan=True, cls=None, indent=None, separators=None,
         default=None, sort_keys=False, **kw):
    """Serialize *obj* as JSON to the write()-supporting *fp*."""
    if (not skipkeys and ensure_ascii and check_circular and allow_nan
            and cls is None and indent is None and separators is None
            and default is None and not sort_keys and not kw):
        iterable = _default_encoder.iterencode(obj)
    else:
        if cls is None:
            cls = JSONEncoder
        iterable = cls(
            skipkeys=skipkeys,
            ensure_ascii=ensure_ascii,
            check_circular=check_circular,
            allow_nan=allow_nan,
            indent=indent,
            separators=separators,
            default=default,
            sort_keys=sort_keys,
            **kw,
        ).iterencode(obj)
    for chunk in iterable:
        fp.write(chunk)


# --------------------------------------------------------------------------- #
# Decoder
# --------------------------------------------------------------------------- #

_WHITESPACE = ' \t\n\r'

_BACKSLASH = {
    '"': '"',
    '\\': '\\',
    '/': '/',
    'b': '\b',
    'f': '\f',
    'n': '\n',
    'r': '\r',
    't': '\t',
}


def _skip_ws(s, idx):
    n = len(s)
    while idx < n and s[idx] in _WHITESPACE:
        idx += 1
    return idx


def _parse_string(s, idx, strict=True):
    # idx points at the opening quote.
    begin = idx
    idx += 1
    chunks = []
    n = len(s)
    while True:
        start = idx
        while idx < n:
            ch = s[idx]
            if ch == '"' or ch == '\\':
                break
            if strict and ch < '\x20':
                raise JSONDecodeError(
                    'Invalid control character %r at' % ch, s, idx)
            idx += 1
        else:
            raise JSONDecodeError(
                'Unterminated string starting at', s, begin)
        chunks.append(s[start:idx])
        ch = s[idx]
        if ch == '"':
            idx += 1
            return ''.join(chunks), idx
        idx += 1
        if idx >= n:
            raise JSONDecodeError(
                'Unterminated string starting at', s, begin)
        esc = s[idx]
        if esc == 'u':
            uni = _parse_unicode(s, idx + 1)
            idx += 5
            if 0xd800 <= uni <= 0xdbff and s[idx:idx + 2] == '\\u':
                uni2 = _parse_unicode(s, idx + 2)
                if 0xdc00 <= uni2 <= 0xdfff:
                    uni = 0x10000 + (((uni - 0xd800) << 10) | (uni2 - 0xdc00))
                    idx += 6
            chunks.append(chr(uni))
        else:
            mapped = _BACKSLASH.get(esc)
            if mapped is None:
                raise JSONDecodeError(
                    'Invalid \\escape: %r' % esc, s, idx)
            chunks.append(mapped)
            idx += 1


def _parse_unicode(s, idx):
    esc = s[idx:idx + 4]
    if len(esc) == 4 and esc[1] not in 'xX':
        try:
            return int(esc, 16)
        except ValueError:
            pass
    raise JSONDecodeError('Invalid \\uXXXX escape', s, idx - 1)


# CPython's ``json/decoder.py`` builds these once at import and hands the *same*
# object back for every ``NaN`` / ``Infinity`` token, so
# ``json.loads('NaN') is json.loads('NaN')`` is True and
# ``len(set(json.loads('[NaN, NaN]')))`` is 1.
NaN = float('nan')
PosInf = float('inf')
NegInf = float('-inf')

_CONSTANTS = {
    '-Infinity': NegInf,
    'Infinity': PosInf,
    'NaN': NaN,
}


def _scan_number(s, idx):
    """Match a RFC 8259 JSON number starting at *idx*.

    Returns ``(numstr, is_float, end_idx)`` or ``None`` if *s[idx]* does not
    start a valid JSON number.
    """
    n = len(s)
    start = idx
    if idx < n and s[idx] == '-':
        idx += 1
    if idx >= n:
        return None
    if s[idx] == '0':
        idx += 1
    elif '1' <= s[idx] <= '9':
        idx += 1
        while idx < n and '0' <= s[idx] <= '9':
            idx += 1
    else:
        return None
    is_float = False
    if idx < n and s[idx] == '.':
        if idx + 1 < n and '0' <= s[idx + 1] <= '9':
            is_float = True
            idx += 2
            while idx < n and '0' <= s[idx] <= '9':
                idx += 1
    if idx < n and (s[idx] == 'e' or s[idx] == 'E'):
        j = idx + 1
        if j < n and (s[j] == '+' or s[j] == '-'):
            j += 1
        if j < n and '0' <= s[j] <= '9':
            is_float = True
            idx = j + 1
            while idx < n and '0' <= s[idx] <= '9':
                idx += 1
    return s[start:idx], is_float, idx


class JSONDecoder:
    """Simple JSON decoder."""

    __module__ = "json.decoder"

    def __init__(self, *, object_hook=None, parse_float=None,
                 parse_int=None, parse_constant=None, strict=True,
                 object_pairs_hook=None):
        self.object_hook = object_hook
        self.parse_float = parse_float or float
        self.parse_int = parse_int or int
        self.parse_constant = parse_constant
        self.strict = strict
        self.object_pairs_hook = object_pairs_hook

    def _parse_value(self, s, idx):
        n = len(s)
        if idx >= n:
            raise JSONDecodeError('Expecting value', s, idx)
        ch = s[idx]
        if ch == '"':
            return _parse_string(s, idx, self.strict)
        if ch == '{':
            return self._parse_object(s, idx)
        if ch == '[':
            return self._parse_array(s, idx)
        if ch == 'n' and s[idx:idx + 4] == 'null':
            return None, idx + 4
        if ch == 't' and s[idx:idx + 4] == 'true':
            return True, idx + 4
        if ch == 'f' and s[idx:idx + 5] == 'false':
            return False, idx + 5
        if ch == 'N' and s[idx:idx + 3] == 'NaN':
            val = (self.parse_constant('NaN')
                   if self.parse_constant is not None else NaN)
            return val, idx + 3
        if ch == 'I' and s[idx:idx + 8] == 'Infinity':
            val = (self.parse_constant('Infinity')
                   if self.parse_constant is not None else PosInf)
            return val, idx + 8
        if ch == '-' and s[idx:idx + 9] == '-Infinity':
            val = (self.parse_constant('-Infinity')
                   if self.parse_constant is not None else NegInf)
            return val, idx + 9
        if ch == '-' or ('0' <= ch <= '9'):
            scanned = _scan_number(s, idx)
            if scanned is not None:
                numstr, is_float, end = scanned
                if is_float:
                    return self.parse_float(numstr), end
                return self.parse_int(numstr), end
        raise JSONDecodeError('Expecting value', s, idx)

    def _parse_array(self, s, idx):
        values = []
        n = len(s)
        idx = _skip_ws(s, idx + 1)
        if idx < n and s[idx] == ']':
            return values, idx + 1
        while True:
            value, idx = self._parse_value(s, idx)
            values.append(value)
            idx = _skip_ws(s, idx)
            if idx >= n:
                raise JSONDecodeError("Expecting ',' delimiter", s, idx)
            ch = s[idx]
            if ch == ']':
                return values, idx + 1
            if ch != ',':
                raise JSONDecodeError("Expecting ',' delimiter", s, idx)
            idx = _skip_ws(s, idx + 1)

    def _parse_object(self, s, idx):
        pairs = []
        n = len(s)
        idx = _skip_ws(s, idx + 1)
        if idx < n and s[idx] == '}':
            if self.object_pairs_hook is not None:
                return self.object_pairs_hook(pairs), idx + 1
            obj = {}
            if self.object_hook is not None:
                return self.object_hook(obj), idx + 1
            return obj, idx + 1
        while True:
            if idx >= n or s[idx] != '"':
                raise JSONDecodeError(
                    'Expecting property name enclosed in double quotes', s, idx)
            key, idx = _parse_string(s, idx, self.strict)
            idx = _skip_ws(s, idx)
            if idx >= n or s[idx] != ':':
                raise JSONDecodeError("Expecting ':' delimiter", s, idx)
            idx = _skip_ws(s, idx + 1)
            value, idx = self._parse_value(s, idx)
            pairs.append((key, value))
            idx = _skip_ws(s, idx)
            if idx >= n:
                raise JSONDecodeError("Expecting ',' delimiter", s, idx)
            ch = s[idx]
            if ch == '}':
                if self.object_pairs_hook is not None:
                    return self.object_pairs_hook(pairs), idx + 1
                obj = dict(pairs)
                if self.object_hook is not None:
                    return self.object_hook(obj), idx + 1
                return obj, idx + 1
            if ch != ',':
                raise JSONDecodeError("Expecting ',' delimiter", s, idx)
            idx = _skip_ws(s, idx + 1)

    def decode(self, s):
        """Return the Python representation of ``s`` (a ``str`` instance
        containing a JSON document)."""
        idx = _skip_ws(s, 0)
        obj, end = self.raw_decode(s, idx=idx)
        end = _skip_ws(s, end)
        if end != len(s):
            raise JSONDecodeError('Extra data', s, end)
        return obj

    def raw_decode(self, s, idx=0):
        """Decode a JSON document from ``s`` beginning at ``idx`` and return a
        2-tuple of the Python representation and the index in ``s`` where the
        document ended."""
        return self._parse_value(s, idx)


_default_decoder = JSONDecoder(object_hook=None, object_pairs_hook=None)


def loads(s, *, cls=None, object_hook=None, parse_float=None,
          parse_int=None, parse_constant=None, object_pairs_hook=None, **kw):
    """Deserialize *s* (a ``str``, ``bytes`` or ``bytearray`` instance
    containing a JSON document) to a Python object."""
    if isinstance(s, str):
        if s.startswith('\ufeff'):
            raise JSONDecodeError(
                'Unexpected UTF-8 BOM (decode using utf-8-sig)', s, 0)
    else:
        if not isinstance(s, (bytes, bytearray)):
            raise TypeError(
                'the JSON object must be str, bytes or bytearray, not %s'
                % s.__class__.__name__)
        s = s.decode(detect_encoding(s), 'surrogatepass')

    if (cls is None and object_hook is None and parse_int is None
            and parse_float is None and parse_constant is None
            and object_pairs_hook is None and not kw):
        return _default_decoder.decode(s)
    if cls is None:
        cls = JSONDecoder
    if object_hook is not None:
        kw['object_hook'] = object_hook
    if object_pairs_hook is not None:
        kw['object_pairs_hook'] = object_pairs_hook
    if parse_float is not None:
        kw['parse_float'] = parse_float
    if parse_int is not None:
        kw['parse_int'] = parse_int
    if parse_constant is not None:
        kw['parse_constant'] = parse_constant
    return cls(**kw).decode(s)


def load(fp, *, cls=None, object_hook=None, parse_float=None,
         parse_int=None, parse_constant=None, object_pairs_hook=None, **kw):
    """Deserialize *fp* (a ``.read()``-supporting file) to a Python object."""
    return loads(
        fp.read(),
        cls=cls,
        object_hook=object_hook,
        parse_float=parse_float,
        parse_int=parse_int,
        parse_constant=parse_constant,
        object_pairs_hook=object_pairs_hook,
        **kw,
    )
