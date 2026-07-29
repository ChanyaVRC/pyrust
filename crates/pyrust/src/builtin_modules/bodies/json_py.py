"""
JSON encoder/decoder — pyrust port of CPython 3.12's ``json`` package.

A minimal but faithful subset: ``dumps`` / ``loads`` plus the
``JSONDecodeError`` exception.  Behaviour (output formatting, type mapping,
escape handling, error wording) targets CPython 3.12 (issue #2620).

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


def _float_repr(o):
    if o != o:
        return 'NaN'
    if o == float('inf'):
        return 'Infinity'
    if o == float('-inf'):
        return '-Infinity'
    return repr(o)


def dumps(obj, *, skipkeys=False, ensure_ascii=True, indent=None,
          separators=None, default=None, sort_keys=False):
    """Serialize *obj* to a JSON formatted ``str``."""
    if separators is not None:
        item_separator, key_separator = separators
    elif indent is not None:
        item_separator, key_separator = ',', ': '
    else:
        item_separator, key_separator = ', ', ': '

    if isinstance(indent, str):
        indent_str = indent
    elif indent is not None:
        indent_str = ' ' * indent
    else:
        indent_str = None

    out = []

    def newline_indent(depth):
        if indent_str is None:
            return ''
        return '\n' + indent_str * depth

    def encode(o, depth):
        if o is True:
            out.append('true')
        elif o is False:
            out.append('false')
        elif o is None:
            out.append('null')
        elif isinstance(o, str):
            out.append(_encode_string(o, ensure_ascii))
        elif isinstance(o, int):
            out.append(int.__repr__(o))
        elif isinstance(o, float):
            out.append(_float_repr(o))
        elif isinstance(o, (list, tuple)):
            encode_list(o, depth)
        elif isinstance(o, dict):
            encode_dict(o, depth)
        elif default is not None:
            encode(default(o), depth)
        else:
            raise TypeError(
                'Object of type %s is not JSON serializable'
                % type(o).__name__)

    def encode_list(lst, depth):
        if not lst:
            out.append('[]')
            return
        out.append('[')
        nl = newline_indent(depth + 1)
        first = True
        for item in lst:
            if first:
                first = False
            else:
                out.append(item_separator)
            out.append(nl)
            encode(item, depth + 1)
        out.append(newline_indent(depth))
        out.append(']')

    def encode_key(key):
        if isinstance(key, str):
            return key
        if key is True:
            return 'true'
        if key is False:
            return 'false'
        if key is None:
            return 'null'
        if isinstance(key, int):
            return int.__repr__(key)
        if isinstance(key, float):
            return _float_repr(key)
        if skipkeys:
            return None
        raise TypeError(
            'keys must be str, int, float, bool or None, not %s'
            % type(key).__name__)

    def encode_dict(dct, depth):
        if not dct:
            out.append('{}')
            return
        items = list(dct.items())
        if sort_keys:
            items.sort(key=lambda kv: kv[0])
        out.append('{')
        nl = newline_indent(depth + 1)
        first = True
        for key, value in items:
            ekey = encode_key(key)
            if ekey is None:
                continue
            if first:
                first = False
            else:
                out.append(item_separator)
            out.append(nl)
            out.append(_encode_string(ekey, ensure_ascii))
            out.append(key_separator)
            encode(value, depth + 1)
        out.append(newline_indent(depth))
        out.append('}')

    encode(obj, 0)
    return ''.join(out)


def dump(obj, fp, **kwargs):
    """Serialize *obj* as JSON to the write()-supporting *fp*."""
    fp.write(dumps(obj, **kwargs))


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
    while idx < len(s) and s[idx] in _WHITESPACE:
        idx += 1
    return idx


def _parse_string(s, idx):
    # idx points at the opening quote.
    begin = idx
    idx += 1
    chunks = []
    n = len(s)
    while True:
        # Scan to the next backslash or closing quote.
        start = idx
        while idx < n:
            ch = s[idx]
            if ch == '"' or ch == '\\':
                break
            if ch < '\x20':
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
        # Backslash escape.
        idx += 1
        if idx >= n:
            raise JSONDecodeError(
                'Unterminated string starting at', s, begin)
        esc = s[idx]
        if esc == 'u':
            uni = _parse_unicode(s, idx + 1)
            idx += 5
            # Surrogate pair.
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


_NUMBER_CHARS = '0123456789+-.eE'

# CPython's ``json/decoder.py`` builds these once at import and hands the *same*
# object back for every ``NaN`` / ``Infinity`` token, so
# ``json.loads('NaN') is json.loads('NaN')`` is True and
# ``len(set(json.loads('[NaN, NaN]')))`` is 1.  That is only observable for NaN,
# now that each ``float('nan')`` mints its own object identity (#2911) — minting
# one per token would put every parsed NaN in its own dict/set slot.
NaN = float('nan')
PosInf = float('inf')
NegInf = float('-inf')


def _parse_number(s, idx):
    start = idx
    n = len(s)
    while idx < n and s[idx] in _NUMBER_CHARS:
        idx += 1
    numstr = s[start:idx]
    if not numstr:
        raise JSONDecodeError('Expecting value', s, start)
    is_float = '.' in numstr or 'e' in numstr or 'E' in numstr
    try:
        if is_float:
            value = float(numstr)
        else:
            value = int(numstr)
    except ValueError:
        raise JSONDecodeError('Expecting value', s, start)
    return value, idx


def _parse_value(s, idx):
    idx = _skip_ws(s, idx)
    if idx >= len(s):
        raise JSONDecodeError('Expecting value', s, idx)
    ch = s[idx]
    if ch == '"':
        return _parse_string(s, idx)
    if ch == '{':
        return _parse_object(s, idx)
    if ch == '[':
        return _parse_array(s, idx)
    if s[idx:idx + 4] == 'null':
        return None, idx + 4
    if s[idx:idx + 4] == 'true':
        return True, idx + 4
    if s[idx:idx + 5] == 'false':
        return False, idx + 5
    if s[idx:idx + 3] == 'NaN':
        return NaN, idx + 3
    if s[idx:idx + 8] == 'Infinity':
        return PosInf, idx + 8
    if s[idx:idx + 9] == '-Infinity':
        return NegInf, idx + 9
    if ch in '-0123456789':
        return _parse_number(s, idx)
    raise JSONDecodeError('Expecting value', s, idx)


def _parse_array(s, idx):
    # idx points at '['.
    values = []
    idx = _skip_ws(s, idx + 1)
    if idx < len(s) and s[idx] == ']':
        return values, idx + 1
    while True:
        value, idx = _parse_value(s, idx)
        values.append(value)
        idx = _skip_ws(s, idx)
        if idx >= len(s):
            raise JSONDecodeError(
                "Expecting ',' delimiter", s, idx)
        ch = s[idx]
        if ch == ']':
            return values, idx + 1
        if ch != ',':
            raise JSONDecodeError(
                "Expecting ',' delimiter", s, idx)
        idx = _skip_ws(s, idx + 1)


def _parse_object(s, idx):
    # idx points at '{'.
    obj = {}
    idx = _skip_ws(s, idx + 1)
    if idx < len(s) and s[idx] == '}':
        return obj, idx + 1
    while True:
        idx = _skip_ws(s, idx)
        if idx >= len(s) or s[idx] != '"':
            raise JSONDecodeError(
                'Expecting property name enclosed in double quotes', s, idx)
        key, idx = _parse_string(s, idx)
        idx = _skip_ws(s, idx)
        if idx >= len(s) or s[idx] != ':':
            raise JSONDecodeError("Expecting ':' delimiter", s, idx)
        idx = _skip_ws(s, idx + 1)
        value, idx = _parse_value(s, idx)
        obj[key] = value
        idx = _skip_ws(s, idx)
        if idx >= len(s):
            raise JSONDecodeError("Expecting ',' delimiter", s, idx)
        ch = s[idx]
        if ch == '}':
            return obj, idx + 1
        if ch != ',':
            raise JSONDecodeError("Expecting ',' delimiter", s, idx)
        idx += 1


def loads(s):
    """Deserialize *s* (a ``str`` containing a JSON document)."""
    if not isinstance(s, str):
        raise TypeError(
            'the JSON object must be str, bytes or bytearray, not %s'
            % type(s).__name__)
    value, end = _parse_value(s, 0)
    end = _skip_ws(s, end)
    if end != len(s):
        raise JSONDecodeError('Extra data', s, end)
    return value


def load(fp):
    """Deserialize *fp* (a ``.read()``-supporting file) to a Python object."""
    return loads(fp.read())
