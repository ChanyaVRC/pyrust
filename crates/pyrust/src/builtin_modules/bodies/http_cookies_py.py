"""HTTP state management — cookies."""

import re
import string
import types

__all__ = ["CookieError", "BaseCookie", "SimpleCookie"]

_nulljoin = ''.join
_semispacejoin = '; '.join
_spacejoin = ' '.join


class CookieError(Exception):
    pass


_LegalChars = string.ascii_letters + string.digits + "!#$%&'*+-.^_`|~:"
_UnescapedChars = _LegalChars + ' ()/<=>?@[]{}'

_Translator = {
    n: '\\%03o' % n
    for n in set(range(256)) - set(map(ord, _UnescapedChars))
}
_Translator.update({
    ord('"'): '\\"',
    ord('\\'): '\\\\',
})

_is_legal_key = re.compile('[%s]+' % re.escape(_LegalChars)).fullmatch
_control_character_re = re.compile(r'[\x00-\x1F\x7F]')


def _has_control_character(*val):
    """Detects control characters within a value.
    Supports any type, as header values can be any type.
    """
    return any(_control_character_re.search(str(v)) for v in val)


def _quote(str):
    r"""Quote a string for use in a cookie header.

    If the string does not need to be double-quoted, then just return the
    string.  Otherwise, surround the string in doublequotes and quote
    (with a \) special characters.
    """
    if str is None or _is_legal_key(str):
        return str
    else:
        return '"' + str.translate(_Translator) + '"'


_unquote_sub = re.compile(r'\\(?:([0-3][0-7][0-7])|(.))').sub


def _unquote_replace(m):
    if m.group(1):
        return chr(int(m.group(1), 8))
    else:
        return m.group(2)


def _unquote(str):
    if str is None or len(str) < 2:
        return str
    if str[0] != '"' or str[-1] != '"':
        return str

    str = str[1:-1]
    return _unquote_sub(_unquote_replace, str)


_weekdayname = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

_monthname = [
    None,
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
]


def _getdate(future=0, weekdayname=_weekdayname, monthname=_monthname):
    from time import gmtime, time
    now = time()
    year, month, day, hh, mm, ss, wd, y, z = gmtime(now + future)
    return "%s, %02d %3s %4d %02d:%02d:%02d GMT" % (
        weekdayname[wd], day, monthname[month], year, hh, mm, ss
    )


class Morsel(dict):
    """A class to hold ONE (key, value) pair.

    In a cookie, each such pair may have several attributes, so this class is
    used to keep the attributes associated with the appropriate key,value pair.
    This class also includes a coded_value attribute, which is used to hold
    the network representation of the value.
    """

    _reserved = {
        "expires": "expires",
        "path": "Path",
        "comment": "Comment",
        "domain": "Domain",
        "max-age": "Max-Age",
        "secure": "Secure",
        "httponly": "HttpOnly",
        "version": "Version",
        "samesite": "SameSite",
    }

    _flags = {'secure', 'httponly'}

    def __init__(self):
        self._key = self._value = self._coded_value = None
        for key in self._reserved:
            dict.__setitem__(self, key, "")

    @property
    def key(self):
        return self._key

    @property
    def value(self):
        return self._value

    @property
    def coded_value(self):
        return self._coded_value

    def __setitem__(self, K, V):
        K = K.lower()
        if K not in self._reserved:
            raise CookieError("Invalid attribute %r" % (K,))
        if _has_control_character(K, V):
            raise CookieError(
                f"Control characters are not allowed in cookies {K!r} {V!r}"
            )
        dict.__setitem__(self, K, V)

    def setdefault(self, key, val=None):
        key = key.lower()
        if key not in self._reserved:
            raise CookieError("Invalid attribute %r" % (key,))
        if _has_control_character(key, val):
            raise CookieError(
                "Control characters are not allowed in cookies %r %r"
                % (key, val)
            )
        return dict.setdefault(self, key, val)

    def __eq__(self, morsel):
        if not isinstance(morsel, Morsel):
            return NotImplemented
        return (
            dict.__eq__(self, morsel)
            and self._value == morsel._value
            and self._key == morsel._key
            and self._coded_value == morsel._coded_value
        )

    def __ne__(self, morsel):
        res = self.__eq__(morsel)
        if res is NotImplemented:
            return NotImplemented
        return not res

    def copy(self):
        morsel = Morsel()
        dict.update(morsel, self)
        morsel._key = self._key
        morsel._value = self._value
        morsel._coded_value = self._coded_value
        return morsel

    def update(self, values):
        data = {}
        for key, val in dict(values).items():
            key = key.lower()
            if key not in self._reserved:
                raise CookieError("Invalid attribute %r" % (key,))
            data[key] = val
        dict.update(self, data)

    def isReservedKey(self, K):
        return K.lower() in self._reserved

    def set(self, key, val, coded_val):
        if key.lower() in self._reserved:
            raise CookieError('Attempt to set a reserved key %r' % (key,))
        if not _is_legal_key(key):
            raise CookieError('Illegal key %r' % (key,))
        if _has_control_character(key, val, coded_val):
            raise CookieError(
                "Control characters are not allowed in cookies %r %r %r"
                % (key, val, coded_val)
            )

        self._key = key
        self._value = val
        self._coded_value = coded_val

    def __getstate__(self):
        return {
            'key': self._key,
            'value': self._value,
            'coded_value': self._coded_value,
        }

    def __setstate__(self, state):
        self._key = state['key']
        self._value = state['value']
        self._coded_value = state['coded_value']

    def output(self, attrs=None, header="Set-Cookie:"):
        return "%s %s" % (header, self.OutputString(attrs))

    __str__ = output

    def __repr__(self):
        return '<%s: %s>' % (self.__class__.__name__, self.OutputString())

    def js_output(self, attrs=None):
        return """
        <script type="text/javascript">
        <!-- begin hiding
        document.cookie = \"%s\";
        // end hiding -->
        </script>
        """ % (self.OutputString(attrs).replace('"', r'\"'))

    def OutputString(self, attrs=None):
        result = []
        append = result.append

        append("%s=%s" % (self.key, self.coded_value))

        if attrs is None:
            attrs = self._reserved
        items = sorted(self.items())
        for key, value in items:
            if value == "":
                continue
            if key not in attrs:
                continue
            if key == "expires" and isinstance(value, int):
                append("%s=%s" % (self._reserved[key], _getdate(value)))
            elif key == "max-age" and isinstance(value, int):
                append("%s=%d" % (self._reserved[key], value))
            elif key == "comment" and isinstance(value, str):
                append("%s=%s" % (self._reserved[key], _quote(value)))
            elif key in self._flags:
                if value:
                    append(str(self._reserved[key]))
            else:
                append("%s=%s" % (self._reserved[key], value))

        return _semispacejoin(result)

    __class_getitem__ = classmethod(types.GenericAlias)


_LegalKeyChars = r"\w\d!#%&'~_`><@,:/\$\*\+\-\.\^\|\)\(\?\}\{\="
_LegalValueChars = _LegalKeyChars + r'\[\]'
_CookiePattern = re.compile(r"""
    \s*                            # Optional whitespace at start of cookie
    (?P<key>                       # Start of group 'key'
    [""" + _LegalKeyChars + r"""]+?   # Any word of at least one letter
    )                              # End of group 'key'
    (                              # Optional group: there may not be a value.
    \s*=\s*                          # Equal Sign
    (?P<val>                         # Start of group 'val'
    "(?:[^\\"]|\\.)*"                  # Any double-quoted string
    |                                  # or
    # Special case for "expires" attr
    (\w{3,6}day|\w{3}),\s              # Day of the week or abbreviated day
    [\w\d\s-]{9,11}\s[\d:]{8}\sGMT     # Date and time in specific format
    |                                  # or
    [""" + _LegalValueChars + r"""]*      # Any word or empty string
    )                                # End of group 'val'
    )?                             # End of optional value group
    \s*                            # Any number of spaces.
    (\s+|;|$)                      # Ending either at space, semicolon, or EOS.
    """, re.ASCII | re.VERBOSE)


class BaseCookie(dict):
    """A container class for a set of Morsels."""

    def value_decode(self, val):
        return val, val

    def value_encode(self, val):
        strval = str(val)
        return strval, strval

    def __init__(self, input=None):
        if input:
            self.load(input)

    def __set(self, key, real_value, coded_value):
        M = self.get(key, Morsel())
        M.set(key, real_value, coded_value)
        dict.__setitem__(self, key, M)

    def __setitem__(self, key, value):
        if isinstance(value, Morsel):
            dict.__setitem__(self, key, value)
        else:
            rval, cval = self.value_encode(value)
            self.__set(key, rval, cval)

    def output(self, attrs=None, header="Set-Cookie:", sep="\015\012"):
        result = []
        items = sorted(self.items())
        for key, value in items:
            value_output = value.output(attrs, header)
            if _has_control_character(value_output):
                raise CookieError("Control characters are not allowed in cookies")
            result.append(value_output)
        return sep.join(result)

    __str__ = output

    def __repr__(self):
        l = []
        items = sorted(self.items())
        for key, value in items:
            l.append('%s=%s' % (key, repr(value.value)))
        return '<%s: %s>' % (self.__class__.__name__, _spacejoin(l))

    def js_output(self, attrs=None):
        result = []
        items = sorted(self.items())
        for key, value in items:
            result.append(value.js_output(attrs))
        return _nulljoin(result)

    def load(self, rawdata):
        if isinstance(rawdata, str):
            self.__parse_string(rawdata)
        else:
            for key, value in rawdata.items():
                self[key] = value
        return

    def __parse_string(self, str, patt=_CookiePattern):
        i = 0
        n = len(str)
        parsed_items = []
        morsel_seen = False

        TYPE_ATTRIBUTE = 1
        TYPE_KEYVALUE = 2

        while 0 <= i < n:
            match = patt.match(str, i)
            if not match:
                break

            key, value = match.group("key"), match.group("val")
            i = match.end(0)

            if key[0] == "$":
                if not morsel_seen:
                    continue
                parsed_items.append((TYPE_ATTRIBUTE, key[1:], value))
            elif key.lower() in Morsel._reserved:
                if not morsel_seen:
                    return
                if value is None:
                    if key.lower() in Morsel._flags:
                        parsed_items.append((TYPE_ATTRIBUTE, key, True))
                    else:
                        return
                else:
                    parsed_items.append((TYPE_ATTRIBUTE, key, _unquote(value)))
            elif value is not None:
                parsed_items.append((TYPE_KEYVALUE, key, self.value_decode(value)))
                morsel_seen = True
            else:
                return

        M = None
        for tp, key, value in parsed_items:
            if tp == TYPE_ATTRIBUTE:
                assert M is not None
                M[key] = value
            else:
                assert tp == TYPE_KEYVALUE
                rval, cval = value
                self.__set(key, rval, cval)
                M = self[key]


class SimpleCookie(BaseCookie):
    """SimpleCookie supports strings as cookie values."""

    def value_decode(self, val):
        return _unquote(val), val

    def value_encode(self, val):
        strval = str(val)
        return strval, _quote(strval)
