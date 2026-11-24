# `tomllib` — Parse TOML 1.0.0 (PEP 680, issue #3184).
#
# Pure-Python TOML 1.0.0 parser targeting CPython 3.12's `tomllib` API:
# `loads(s, /, *, parse_float=float)`, `load(fp, /, *, parse_float=float)`,
# and `TOMLDecodeError`.

from datetime import date as _date
from datetime import datetime as _datetime
from datetime import time as _time
from datetime import timedelta as _timedelta
from datetime import timezone as _timezone

__all__ = ("loads", "load", "TOMLDecodeError")

_ASCII_CTRL = frozenset(chr(i) for i in range(32)) | frozenset((chr(127),))

_ILLEGAL_BASIC_STR_CHARS = _ASCII_CTRL - frozenset("\t")
_ILLEGAL_MULTILINE_BASIC_STR_CHARS = _ASCII_CTRL - frozenset("\t\n")
_ILLEGAL_LITERAL_STR_CHARS = _ILLEGAL_BASIC_STR_CHARS
_ILLEGAL_MULTILINE_LITERAL_STR_CHARS = _ILLEGAL_MULTILINE_BASIC_STR_CHARS
_ILLEGAL_COMMENT_CHARS = _ILLEGAL_BASIC_STR_CHARS

_TOML_WS = frozenset(" \t")
_TOML_WS_AND_NEWLINE = _TOML_WS | frozenset("\n")
_BARE_KEY_CHARS = frozenset(
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
)
_KEY_INITIAL_CHARS = _BARE_KEY_CHARS | frozenset("\"'")
_HEXDIGIT_CHARS = frozenset("abcdefABCDEF0123456789")
_DEC_DIGITS = frozenset("0123456789")
_OCT_DIGITS = frozenset("01234567")
_BIN_DIGITS = frozenset("01")

_BASIC_STR_ESCAPE_REPLACEMENTS = {
    "\\b": "\u0008",
    "\\t": "\u0009",
    "\\n": "\u000A",
    "\\f": "\u000C",
    "\\r": "\u000D",
    '\\"': "\u0022",
    "\\\\": "\u005C",
}

_TZ_CACHE = {}


class TOMLDecodeError(ValueError):
    """An error raised if a document is not valid TOML."""


def _suffixed_err(src, pos, msg):
    """Return a `TOMLDecodeError` with line/column coordinate suffix matching
    CPython 3.12's `tomllib`."""

    def coord_repr(src, pos):
        if pos >= len(src):
            return "end of document"
        line = src.count("\n", 0, pos) + 1
        if line == 1:
            column = pos + 1
        else:
            column = pos - src.rindex("\n", 0, pos)
        return f"line {line}, column {column}"

    return TOMLDecodeError(f"{msg} (at {coord_repr(src, pos)})")


def _cached_tz(hour_str, minute_str, sign_str):
    key = (hour_str, minute_str, sign_str)
    tz = _TZ_CACHE.get(key)
    if tz is None:
        sign = 1 if sign_str == "+" else -1
        tz = _timezone(
            _timedelta(
                hours=sign * int(hour_str),
                minutes=sign * int(minute_str),
            )
        )
        _TZ_CACHE[key] = tz
    return tz


def _read_two_digits(src, pos, lo, hi):
    if pos + 2 > len(src):
        return None
    c0 = src[pos]
    c1 = src[pos + 1]
    if c0 not in _DEC_DIGITS or c1 not in _DEC_DIGITS:
        return None
    val = int(src[pos : pos + 2])
    if lo <= val <= hi:
        return val
    return None


def _match_time_parts(src, pos):
    """Match HH:MM:SS(.frac)? at `pos`. Returns (end_pos, h, m, s, micros) or None."""
    n = len(src)
    if pos + 8 > n:
        return None
    if src[pos + 2] != ":" or src[pos + 5] != ":":
        return None
    hour = _read_two_digits(src, pos, 0, 23)
    if hour is None:
        return None
    minute = _read_two_digits(src, pos + 3, 0, 59)
    if minute is None:
        return None
    sec = _read_two_digits(src, pos + 6, 0, 59)
    if sec is None:
        return None
    cur = pos + 8
    micros = 0
    if cur + 1 < n and src[cur] == "." and src[cur + 1] in _DEC_DIGITS:
        cur += 1
        frac_start = cur
        while cur < n and src[cur] in _DEC_DIGITS:
            cur += 1
        frac_str = src[frac_start : min(cur, frac_start + 6)]
        micros = int(frac_str.ljust(6, "0"))
    return cur, hour, minute, sec, micros


def _match_datetime(src, pos):
    """Match RFC 3339 date, local datetime, or offset datetime at `pos`.

    Returns (end_pos, date_or_datetime) or None. May raise ValueError for
    structurally matched but invalid calendar dates (e.g. 2023-02-29).
    """
    n = len(src)
    if pos + 10 > n:
        return None
    if src[pos + 4] != "-" or src[pos + 7] != "-":
        return None
    year_s = src[pos : pos + 4]
    if not (
        year_s[0] in _DEC_DIGITS
        and year_s[1] in _DEC_DIGITS
        and year_s[2] in _DEC_DIGITS
        and year_s[3] in _DEC_DIGITS
    ):
        return None
    month = _read_two_digits(src, pos + 5, 1, 12)
    if month is None:
        return None
    day = _read_two_digits(src, pos + 8, 1, 31)
    if day is None:
        return None
    year = int(year_s)
    date_end = pos + 10

    if date_end < n and src[date_end] in "Tt ":
        time_res = _match_time_parts(src, date_end + 1)
        if time_res is not None:
            t_end, hour, minute, sec, micros = time_res
            tz = None
            end = t_end
            if t_end < n:
                ch = src[t_end]
                if ch == "Z" or ch == "z":
                    tz = _timezone.utc
                    end = t_end + 1
                elif (
                    (ch == "+" or ch == "-")
                    and t_end + 6 <= n
                    and src[t_end + 3] == ":"
                ):
                    oh = _read_two_digits(src, t_end + 1, 0, 23)
                    om = _read_two_digits(src, t_end + 4, 0, 59)
                    if oh is not None and om is not None:
                        tz = _cached_tz(
                            src[t_end + 1 : t_end + 3],
                            src[t_end + 4 : t_end + 6],
                            ch,
                        )
                        end = t_end + 6
            return end, _datetime(year, month, day, hour, minute, sec, micros, tzinfo=tz)

    return date_end, _date(year, month, day)


def _match_localtime(src, pos):
    """Match RFC 3339 local time at `pos`. Returns (end_pos, time) or None."""
    time_res = _match_time_parts(src, pos)
    if time_res is None:
        return None
    end, hour, minute, sec, micros = time_res
    return end, _time(hour, minute, sec, micros)


def _scan_digits(src, pos, digits):
    """Scan `<digit>(?:_?<digit>)*` starting at `pos`. Returns end pos or -1."""
    n = len(src)
    if pos >= n or src[pos] not in digits:
        return -1
    cur = pos + 1
    while cur < n:
        ch = src[cur]
        if ch in digits:
            cur += 1
        elif ch == "_" and cur + 1 < n and src[cur + 1] in digits:
            cur += 2
        else:
            break
    return cur


def _match_number(src, pos, parse_float):
    """Match a TOML integer or finite float at `pos`. Returns (end, val) or None."""
    n = len(src)
    if pos >= n:
        return None

    # Prefixed base-16 / base-8 / base-2 integers (no sign allowed).
    if src[pos] == "0" and pos + 1 < n:
        prefix = src[pos + 1]
        if prefix == "x":
            end = _scan_digits(src, pos + 2, _HEXDIGIT_CHARS)
            if end != -1:
                return end, int(src[pos:end], 0)
        elif prefix == "b":
            end = _scan_digits(src, pos + 2, _BIN_DIGITS)
            if end != -1:
                return end, int(src[pos:end], 0)
        elif prefix == "o":
            end = _scan_digits(src, pos + 2, _OCT_DIGITS)
            if end != -1:
                return end, int(src[pos:end], 0)

    # Decimal integer or float.
    cur = pos
    if src[cur] == "+" or src[cur] == "-":
        cur += 1
        if cur >= n:
            return None

    first = src[cur]
    if first == "0":
        cur += 1
    elif first in "123456789":
        cur = _scan_digits(src, cur, _DEC_DIGITS)
    else:
        return None

    is_float = False

    # Optional fractional part: `.<digit>(?:_?<digit>)*`
    if cur < n and src[cur] == ".":
        frac_end = _scan_digits(src, cur + 1, _DEC_DIGITS)
        if frac_end != -1:
            cur = frac_end
            is_float = True

    # Optional exponent part: `[eE][+-]?<digit>(?:_?<digit>)*`
    if cur < n and (src[cur] == "e" or src[cur] == "E"):
        exp_pos = cur + 1
        if exp_pos < n and (src[exp_pos] == "+" or src[exp_pos] == "-"):
            exp_pos += 1
        exp_end = _scan_digits(src, exp_pos, _DEC_DIGITS)
        if exp_end != -1:
            cur = exp_end
            is_float = True

    raw = src[pos:cur]
    if is_float:
        return cur, parse_float(raw)
    return cur, int(raw, 0)


def load(fp, /, *, parse_float=float):
    """Parse TOML from a binary file object."""
    b = fp.read()
    try:
        s = b.decode()
    except AttributeError:
        raise TypeError(
            "File must be opened in binary mode, e.g. use `open('foo.toml', 'rb')`"
        ) from None
    return loads(s, parse_float=parse_float)


def loads(s, /, *, parse_float=float):
    """Parse TOML from a string."""
    # The spec allows converting "\r\n" to "\n", even in string literals.
    try:
        src = s.replace("\r\n", "\n")
    except (AttributeError, TypeError):
        raise TypeError(
            f"Expected str object, not '{type(s).__qualname__}'"
        ) from None

    pos = 0
    out = _Output()
    header = ()
    parse_float = _make_safe_parse_float(parse_float)

    while True:
        # 1. Skip line leading whitespace
        pos = _skip_chars(src, pos, _TOML_WS)

        # 2. Parse statement
        try:
            char = src[pos]
        except IndexError:
            break
        if char == "\n":
            pos += 1
            continue
        if char in _KEY_INITIAL_CHARS:
            pos = _key_value_rule(src, pos, out, header, parse_float)
            pos = _skip_chars(src, pos, _TOML_WS)
        elif char == "[":
            try:
                second_char = src[pos + 1]
            except IndexError:
                second_char = None
            out.flags.finalize_pending()
            if second_char == "[":
                pos, header = _create_list_rule(src, pos, out)
            else:
                pos, header = _create_dict_rule(src, pos, out)
            pos = _skip_chars(src, pos, _TOML_WS)
        elif char != "#":
            raise _suffixed_err(src, pos, "Invalid statement")

        # 3. Skip comment
        pos = _skip_comment(src, pos)

        # 4. Expect end of line or end of file
        try:
            char = src[pos]
        except IndexError:
            break
        if char != "\n":
            raise _suffixed_err(
                src, pos, "Expected newline or end of document after a statement"
            )
        pos += 1

    return out.data.dict


class _Flags:
    """Flags that map to parsed keys/namespaces."""

    # Marks an immutable namespace (inline array or inline table).
    FROZEN = 0
    # Marks a nest that has been explicitly created and can no longer
    # be opened using the "[table]" syntax.
    EXPLICIT_NEST = 1

    def __init__(self):
        self._flags = {}
        self._pending_flags = set()

    def add_pending(self, key, flag):
        self._pending_flags.add((key, flag))

    def finalize_pending(self):
        for key, flag in self._pending_flags:
            self.set(key, flag, recursive=False)
        self._pending_flags.clear()

    def unset_all(self, key):
        cont = self._flags
        for k in key[:-1]:
            if k not in cont:
                return
            cont = cont[k]["nested"]
        cont.pop(key[-1], None)

    def set(self, key, flag, *, recursive):
        cont = self._flags
        key_parent, key_stem = key[:-1], key[-1]
        for k in key_parent:
            if k not in cont:
                cont[k] = {"flags": set(), "recursive_flags": set(), "nested": {}}
            cont = cont[k]["nested"]
        if key_stem not in cont:
            cont[key_stem] = {"flags": set(), "recursive_flags": set(), "nested": {}}
        cont[key_stem]["recursive_flags" if recursive else "flags"].add(flag)

    def is_(self, key, flag):
        if not key:
            return False
        cont = self._flags
        for k in key[:-1]:
            if k not in cont:
                return False
            inner_cont = cont[k]
            if flag in inner_cont["recursive_flags"]:
                return True
            cont = inner_cont["nested"]
        key_stem = key[-1]
        if key_stem in cont:
            cont = cont[key_stem]
            return flag in cont["flags"] or flag in cont["recursive_flags"]
        return False


class _NestedDict:
    def __init__(self):
        self.dict = {}

    def get_or_create_nest(self, key, *, access_lists=True):
        cont = self.dict
        for k in key:
            if k not in cont:
                cont[k] = {}
            cont = cont[k]
            if access_lists and isinstance(cont, list):
                cont = cont[-1]
            if not isinstance(cont, dict):
                raise KeyError("There is no nest behind this key")
        return cont

    def append_nest_to_list(self, key):
        cont = self.get_or_create_nest(key[:-1])
        last_key = key[-1]
        if last_key in cont:
            list_ = cont[last_key]
            if not isinstance(list_, list):
                raise KeyError("An object other than list found behind this key")
            list_.append({})
        else:
            cont[last_key] = [{}]


class _Output:
    def __init__(self):
        self.data = _NestedDict()
        self.flags = _Flags()


def _skip_chars(src, pos, chars):
    try:
        while src[pos] in chars:
            pos += 1
    except IndexError:
        pass
    return pos


def _skip_until(src, pos, expect, *, error_on, error_on_eof):
    try:
        new_pos = src.index(expect, pos)
    except ValueError:
        new_pos = len(src)
        if error_on_eof:
            raise _suffixed_err(src, new_pos, f"Expected {expect!r}") from None

    if not error_on.isdisjoint(src[pos:new_pos]):
        while src[pos] not in error_on:
            pos += 1
        raise _suffixed_err(src, pos, f"Found invalid character {src[pos]!r}")
    return new_pos


def _skip_comment(src, pos):
    try:
        char = src[pos]
    except IndexError:
        char = None
    if char == "#":
        return _skip_until(
            src, pos + 1, "\n", error_on=_ILLEGAL_COMMENT_CHARS, error_on_eof=False
        )
    return pos


def _skip_comments_and_array_ws(src, pos):
    while True:
        pos_before_skip = pos
        pos = _skip_chars(src, pos, _TOML_WS_AND_NEWLINE)
        pos = _skip_comment(src, pos)
        if pos == pos_before_skip:
            return pos


def _create_dict_rule(src, pos, out):
    pos += 1  # Skip "["
    pos = _skip_chars(src, pos, _TOML_WS)
    pos, key = _parse_key(src, pos)

    if out.flags.is_(key, _Flags.EXPLICIT_NEST) or out.flags.is_(key, _Flags.FROZEN):
        raise _suffixed_err(src, pos, f"Cannot declare {key} twice")
    out.flags.set(key, _Flags.EXPLICIT_NEST, recursive=False)
    try:
        out.data.get_or_create_nest(key)
    except KeyError:
        raise _suffixed_err(src, pos, "Cannot overwrite a value") from None

    if not src.startswith("]", pos):
        raise _suffixed_err(
            src, pos, "Expected ']' at the end of a table declaration"
        )
    return pos + 1, key


def _create_list_rule(src, pos, out):
    pos += 2  # Skip "[["
    pos = _skip_chars(src, pos, _TOML_WS)
    pos, key = _parse_key(src, pos)

    if out.flags.is_(key, _Flags.FROZEN):
        raise _suffixed_err(src, pos, f"Cannot mutate immutable namespace {key}")
    # Free the namespace now that it points to another empty list item...
    out.flags.unset_all(key)
    # ...but this key precisely is still prohibited from table declaration
    out.flags.set(key, _Flags.EXPLICIT_NEST, recursive=False)
    try:
        out.data.append_nest_to_list(key)
    except KeyError:
        raise _suffixed_err(src, pos, "Cannot overwrite a value") from None

    if not src.startswith("]]", pos):
        raise _suffixed_err(
            src, pos, "Expected ']]' at the end of an array declaration"
        )
    return pos + 2, key


def _key_value_rule(src, pos, out, header, parse_float):
    pos, key, value = _parse_key_value_pair(src, pos, parse_float)
    key_parent, key_stem = key[:-1], key[-1]
    abs_key_parent = header + key_parent

    relative_path_cont_keys = (header + key[:i] for i in range(1, len(key)))
    for cont_key in relative_path_cont_keys:
        if out.flags.is_(cont_key, _Flags.EXPLICIT_NEST):
            raise _suffixed_err(src, pos, f"Cannot redefine namespace {cont_key}")
        out.flags.add_pending(cont_key, _Flags.EXPLICIT_NEST)

    if out.flags.is_(abs_key_parent, _Flags.FROZEN):
        raise _suffixed_err(
            src, pos, f"Cannot mutate immutable namespace {abs_key_parent}"
        )

    try:
        nest = out.data.get_or_create_nest(abs_key_parent)
    except KeyError:
        raise _suffixed_err(src, pos, "Cannot overwrite a value") from None
    if key_stem in nest:
        raise _suffixed_err(src, pos, "Cannot overwrite a value")
    # Mark inline table and array namespaces recursively immutable
    if isinstance(value, (dict, list)):
        out.flags.set(header + key, _Flags.FROZEN, recursive=True)
    nest[key_stem] = value
    return pos


def _parse_key_value_pair(src, pos, parse_float):
    pos, key = _parse_key(src, pos)
    try:
        char = src[pos]
    except IndexError:
        char = None
    if char != "=":
        raise _suffixed_err(src, pos, "Expected '=' after a key in a key/value pair")
    pos += 1
    pos = _skip_chars(src, pos, _TOML_WS)
    pos, value = _parse_value(src, pos, parse_float)
    return pos, key, value


def _parse_key(src, pos):
    pos, key_part = _parse_key_part(src, pos)
    key = (key_part,)
    pos = _skip_chars(src, pos, _TOML_WS)
    while True:
        try:
            char = src[pos]
        except IndexError:
            char = None
        if char != ".":
            return pos, key
        pos += 1
        pos = _skip_chars(src, pos, _TOML_WS)
        pos, key_part = _parse_key_part(src, pos)
        key += (key_part,)
        pos = _skip_chars(src, pos, _TOML_WS)


def _parse_key_part(src, pos):
    try:
        char = src[pos]
    except IndexError:
        char = None
    if char in _BARE_KEY_CHARS:
        start_pos = pos
        pos = _skip_chars(src, pos, _BARE_KEY_CHARS)
        return pos, src[start_pos:pos]
    if char == "'":
        return _parse_literal_str(src, pos)
    if char == '"':
        return _parse_one_line_basic_str(src, pos)
    raise _suffixed_err(src, pos, "Invalid initial character for a key part")


def _parse_one_line_basic_str(src, pos):
    pos += 1
    return _parse_basic_str(src, pos, multiline=False)


def _parse_array(src, pos, parse_float):
    pos += 1
    array = []

    pos = _skip_comments_and_array_ws(src, pos)
    if src.startswith("]", pos):
        return pos + 1, array
    while True:
        pos, val = _parse_value(src, pos, parse_float)
        array.append(val)
        pos = _skip_comments_and_array_ws(src, pos)

        c = src[pos : pos + 1]
        if c == "]":
            return pos + 1, array
        if c != ",":
            raise _suffixed_err(src, pos, "Unclosed array")
        pos += 1

        pos = _skip_comments_and_array_ws(src, pos)
        if src.startswith("]", pos):
            return pos + 1, array


def _parse_inline_table(src, pos, parse_float):
    pos += 1
    nested_dict = _NestedDict()
    flags = _Flags()

    pos = _skip_chars(src, pos, _TOML_WS)
    if src.startswith("}", pos):
        return pos + 1, nested_dict.dict
    while True:
        pos, key, value = _parse_key_value_pair(src, pos, parse_float)
        key_parent, key_stem = key[:-1], key[-1]
        if flags.is_(key, _Flags.FROZEN):
            raise _suffixed_err(src, pos, f"Cannot mutate immutable namespace {key}")
        try:
            nest = nested_dict.get_or_create_nest(key_parent, access_lists=False)
        except KeyError:
            raise _suffixed_err(src, pos, "Cannot overwrite a value") from None
        if key_stem in nest:
            raise _suffixed_err(src, pos, f"Duplicate inline table key {key_stem!r}")
        nest[key_stem] = value
        pos = _skip_chars(src, pos, _TOML_WS)
        c = src[pos : pos + 1]
        if c == "}":
            return pos + 1, nested_dict.dict
        if c != ",":
            raise _suffixed_err(src, pos, "Unclosed inline table")
        if isinstance(value, (dict, list)):
            flags.set(key, _Flags.FROZEN, recursive=True)
        pos += 1
        pos = _skip_chars(src, pos, _TOML_WS)


def _parse_basic_str_escape(src, pos, *, multiline=False):
    escape_id = src[pos : pos + 2]
    pos += 2
    if multiline and escape_id in {"\\ ", "\\\t", "\\\n"}:
        if escape_id != "\\\n":
            pos = _skip_chars(src, pos, _TOML_WS)
            try:
                char = src[pos]
            except IndexError:
                return pos, ""
            if char != "\n":
                raise _suffixed_err(src, pos, "Unescaped '\\' in a string")
            pos += 1
        pos = _skip_chars(src, pos, _TOML_WS_AND_NEWLINE)
        return pos, ""
    if escape_id == "\\u":
        return _parse_hex_char(src, pos, 4)
    if escape_id == "\\U":
        return _parse_hex_char(src, pos, 8)
    try:
        return pos, _BASIC_STR_ESCAPE_REPLACEMENTS[escape_id]
    except KeyError:
        raise _suffixed_err(src, pos, "Unescaped '\\' in a string") from None


def _parse_basic_str_escape_multiline(src, pos):
    return _parse_basic_str_escape(src, pos, multiline=True)


def _parse_hex_char(src, pos, hex_len):
    hex_str = src[pos : pos + hex_len]
    if len(hex_str) != hex_len or not _HEXDIGIT_CHARS.issuperset(hex_str):
        raise _suffixed_err(src, pos, "Invalid hex value")
    pos += hex_len
    hex_int = int(hex_str, 16)
    if not _is_unicode_scalar_value(hex_int):
        raise _suffixed_err(
            src, pos, "Escaped character is not a Unicode scalar value"
        )
    return pos, chr(hex_int)


def _parse_literal_str(src, pos):
    pos += 1  # Skip starting apostrophe
    start_pos = pos
    pos = _skip_until(
        src, pos, "'", error_on=_ILLEGAL_LITERAL_STR_CHARS, error_on_eof=True
    )
    return pos + 1, src[start_pos:pos]  # Skip ending apostrophe


def _parse_multiline_str(src, pos, *, literal):
    pos += 3
    if src.startswith("\n", pos):
        pos += 1

    if literal:
        delim = "'"
        end_pos = _skip_until(
            src,
            pos,
            "'''",
            error_on=_ILLEGAL_MULTILINE_LITERAL_STR_CHARS,
            error_on_eof=True,
        )
        result = src[pos:end_pos]
        pos = end_pos + 3
    else:
        delim = '"'
        pos, result = _parse_basic_str(src, pos, multiline=True)

    # Add at maximum two extra apostrophes/quotes if the end sequence
    # is 4 or 5 chars long instead of just 3.
    if not src.startswith(delim, pos):
        return pos, result
    pos += 1
    if not src.startswith(delim, pos):
        return pos, result + delim
    pos += 1
    return pos, result + (delim * 2)


def _parse_basic_str(src, pos, *, multiline):
    if multiline:
        error_on = _ILLEGAL_MULTILINE_BASIC_STR_CHARS
        parse_escapes = _parse_basic_str_escape_multiline
    else:
        error_on = _ILLEGAL_BASIC_STR_CHARS
        parse_escapes = _parse_basic_str_escape
    result = ""
    start_pos = pos
    while True:
        try:
            char = src[pos]
        except IndexError:
            raise _suffixed_err(src, pos, "Unterminated string") from None
        if char == '"':
            if not multiline:
                return pos + 1, result + src[start_pos:pos]
            if src.startswith('"""', pos):
                return pos + 3, result + src[start_pos:pos]
            pos += 1
            continue
        if char == "\\":
            result += src[start_pos:pos]
            pos, parsed_escape = parse_escapes(src, pos)
            result += parsed_escape
            start_pos = pos
            continue
        if char in error_on:
            raise _suffixed_err(src, pos, f"Illegal character {char!r}")
        pos += 1


def _parse_value(src, pos, parse_float):
    try:
        char = src[pos]
    except IndexError:
        char = None

    # Basic strings
    if char == '"':
        if src.startswith('"""', pos):
            return _parse_multiline_str(src, pos, literal=False)
        return _parse_one_line_basic_str(src, pos)

    # Literal strings
    if char == "'":
        if src.startswith("'''", pos):
            return _parse_multiline_str(src, pos, literal=True)
        return _parse_literal_str(src, pos)

    # Booleans
    if char == "t":
        if src.startswith("true", pos):
            return pos + 4, True
    if char == "f":
        if src.startswith("false", pos):
            return pos + 5, False

    # Arrays
    if char == "[":
        return _parse_array(src, pos, parse_float)

    # Inline tables
    if char == "{":
        return _parse_inline_table(src, pos, parse_float)

    # Dates and times
    try:
        datetime_res = _match_datetime(src, pos)
    except ValueError as e:
        raise _suffixed_err(src, pos, "Invalid date or datetime") from e
    if datetime_res is not None:
        return datetime_res

    localtime_res = _match_localtime(src, pos)
    if localtime_res is not None:
        return localtime_res

    # Integers and finite floats
    number_res = _match_number(src, pos, parse_float)
    if number_res is not None:
        return number_res

    # Special floats
    first_three = src[pos : pos + 3]
    if first_three in {"inf", "nan"}:
        return pos + 3, parse_float(first_three)
    first_four = src[pos : pos + 4]
    if first_four in {"-inf", "+inf", "-nan", "+nan"}:
        return pos + 4, parse_float(first_four)

    raise _suffixed_err(src, pos, "Invalid value")


def _is_unicode_scalar_value(codepoint):
    return (0 <= codepoint <= 55295) or (57344 <= codepoint <= 1114111)


def _make_safe_parse_float(parse_float):
    if parse_float is float:
        return float

    def safe_parse_float(float_str):
        float_value = parse_float(float_str)
        if isinstance(float_value, (dict, list)):
            raise ValueError("parse_float must not return dicts or lists")
        return float_value

    return safe_parse_float
