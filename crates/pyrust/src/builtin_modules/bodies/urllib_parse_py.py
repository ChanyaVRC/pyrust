"""Parse (absolute and relative) URLs."""

from collections import namedtuple
import functools
import math
import re
import types
import warnings

__all__ = ["urlparse", "urlunparse", "urljoin", "urldefrag",
           "urlsplit", "urlunsplit", "urlencode", "parse_qs",
           "parse_qsl", "quote", "quote_plus", "quote_from_bytes",
           "unquote", "unquote_plus", "unquote_to_bytes",
           "DefragResult", "ParseResult", "SplitResult",
           "DefragResultBytes", "ParseResultBytes", "SplitResultBytes"]

# A classification of schemes.
# The empty string classifies URLs with no scheme specified,
# being the default value returned by "urlsplit" and "urlparse".

uses_relative = ['', 'ftp', 'http', 'gopher', 'nntp', 'imap',
                 'wais', 'file', 'https', 'shttp', 'mms',
                 'prospero', 'rtsp', 'rtsps', 'rtspu', 'sftp',
                 'svn', 'svn+ssh', 'ws', 'wss']

uses_netloc = ['', 'ftp', 'http', 'gopher', 'nntp', 'telnet',
               'imap', 'wais', 'file', 'mms', 'https', 'shttp',
               'snews', 'prospero', 'rtsp', 'rtsps', 'rtspu', 'rsync',
               'svn', 'svn+ssh', 'sftp', 'nfs', 'git', 'git+ssh',
               'ws', 'wss', 'itms-services']

uses_params = ['', 'ftp', 'hdl', 'prospero', 'http', 'imap',
               'https', 'shttp', 'rtsp', 'rtsps', 'rtspu', 'sip',
               'sips', 'mms', 'sftp', 'tel']

non_hierarchical = ['gopher', 'hdl', 'mailto', 'news',
                    'telnet', 'wais', 'imap', 'snews', 'sip', 'sips']

uses_query = ['', 'http', 'wais', 'imap', 'https', 'shttp', 'mms',
              'gopher', 'rtsp', 'rtsps', 'rtspu', 'sip', 'sips']

uses_fragment = ['', 'ftp', 'hdl', 'http', 'gopher', 'news',
                 'nntp', 'wais', 'https', 'shttp', 'snews',
                 'file', 'prospero']

# Characters valid in scheme names
scheme_chars = ('abcdefghijklmnopqrstuvwxyz'
                'ABCDEFGHIJKLMNOPQRSTUVWXYZ'
                '0123456789'
                '+-.')

# Leading and trailing C0 control and space to be stripped per WHATWG spec.
_WHATWG_C0_CONTROL_OR_SPACE = (
    '\x00\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\x0c\r\x0e\x0f'
    '\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f '
)

# Unsafe bytes to be removed per WHATWG spec
_UNSAFE_URL_BYTES_TO_REMOVE = ['\t', '\r', '\n']

# Every non-ASCII Unicode codepoint whose NFKC decomposition contains any of
# '/', '?', '#', '@', or ':' (matches CPython's _checknetloc without requiring
# the unicodedata module).
_NFKC_INVALID_NETLOC_CHARS = frozenset((
    '\u2047', '\u2048', '\u2049', '\u2100', '\u2101', '\u2105', '\u2106',
    '\u2a74', '\ufe13', '\ufe16', '\ufe55', '\ufe56', '\ufe5f', '\ufe6b',
    '\uff03', '\uff0f', '\uff1a', '\uff1f', '\uff20',
))

_HEX_CHARS = frozenset('0123456789abcdefABCDEF')


def clear_cache():
    """Clear internal performance caches."""
    urlsplit.cache_clear()
    _byte_quoter_factory.cache_clear()


# Helpers for bytes handling
_implicit_encoding = 'ascii'
_implicit_errors = 'strict'


def _noop(obj):
    return obj


def _encode_result(obj, encoding=_implicit_encoding,
                   errors=_implicit_errors):
    return obj.encode(encoding, errors)


def _decode_args(args, encoding=_implicit_encoding,
                 errors=_implicit_errors):
    return tuple(x.decode(encoding, errors) if x else '' for x in args)


def _coerce_args(*args):
    str_input = isinstance(args[0], str)
    for arg in args[1:]:
        if arg and isinstance(arg, str) != str_input:
            raise TypeError("Cannot mix str and non-str arguments")
    if str_input:
        return args + (_noop,)
    return _decode_args(args) + (_encode_result,)


# Result objects are more helpful than simple tuples
class _ResultMixinStr(object):
    """Standard approach to encoding parsed results from str to bytes"""
    __slots__ = ()

    def encode(self, encoding='ascii', errors='strict'):
        return self._encoded_counterpart(*(x.encode(encoding, errors) for x in self))


class _ResultMixinBytes(object):
    """Standard approach to decoding parsed results from bytes to str"""
    __slots__ = ()

    def decode(self, encoding='ascii', errors='strict'):
        return self._decoded_counterpart(*(x.decode(encoding, errors) for x in self))


class _NetlocResultMixinBase(object):
    """Shared methods for the parsed result objects containing a netloc element"""
    __slots__ = ()

    @property
    def username(self):
        return self._userinfo[0]

    @property
    def password(self):
        return self._userinfo[1]

    @property
    def hostname(self):
        hostname = self._hostinfo[0]
        if not hostname:
            return None
        # Scoped IPv6 address may have zone info, which must not be lowercased
        separator = '%' if isinstance(hostname, str) else b'%'
        hostname, percent, zone = hostname.partition(separator)
        return hostname.lower() + percent + zone

    @property
    def port(self):
        port = self._hostinfo[1]
        if port is not None:
            if port.isdigit() and port.isascii():
                port = int(port)
            else:
                raise ValueError(f"Port could not be cast to integer value as {port!r}")
            if not (0 <= port <= 65535):
                raise ValueError("Port out of range 0-65535")
        return port

    __class_getitem__ = classmethod(types.GenericAlias)


class _NetlocResultMixinStr(_NetlocResultMixinBase, _ResultMixinStr):
    __slots__ = ()

    @property
    def _userinfo(self):
        netloc = self.netloc
        userinfo, have_info, hostinfo = netloc.rpartition('@')
        if have_info:
            username, have_password, password = userinfo.partition(':')
            if not have_password:
                password = None
        else:
            username = password = None
        return username, password

    @property
    def _hostinfo(self):
        netloc = self.netloc
        _, _, hostinfo = netloc.rpartition('@')
        _, have_open_br, bracketed = hostinfo.partition('[')
        if have_open_br:
            hostname, _, port = bracketed.partition(']')
            _, _, port = port.partition(':')
        else:
            hostname, _, port = hostinfo.partition(':')
        if not port:
            port = None
        return hostname, port


class _NetlocResultMixinBytes(_NetlocResultMixinBase, _ResultMixinBytes):
    __slots__ = ()

    @property
    def _userinfo(self):
        netloc = self.netloc
        userinfo, have_info, hostinfo = netloc.rpartition(b'@')
        if have_info:
            username, have_password, password = userinfo.partition(b':')
            if not have_password:
                password = None
        else:
            username = password = None
        return username, password

    @property
    def _hostinfo(self):
        netloc = self.netloc
        _, _, hostinfo = netloc.rpartition(b'@')
        _, have_open_br, bracketed = hostinfo.partition(b'[')
        if have_open_br:
            hostname, _, port = bracketed.partition(b']')
            _, _, port = port.partition(b':')
        else:
            hostname, _, port = hostinfo.partition(b':')
        if not port:
            port = None
        return hostname, port


_DefragResultBase = namedtuple('DefragResult', 'url fragment')
_SplitResultBase = namedtuple(
    'SplitResult', 'scheme netloc path query fragment')
_ParseResultBase = namedtuple(
    'ParseResult', 'scheme netloc path params query fragment')

ResultBase = _NetlocResultMixinStr


# Structured result objects for string data
class DefragResult(_DefragResultBase, _ResultMixinStr):
    __slots__ = ()

    def geturl(self):
        if self.fragment:
            return self.url + '#' + self.fragment
        else:
            return self.url


class SplitResult(_SplitResultBase, _NetlocResultMixinStr):
    __slots__ = ()

    def geturl(self):
        return urlunsplit(self)


class ParseResult(_ParseResultBase, _NetlocResultMixinStr):
    __slots__ = ()

    def geturl(self):
        return urlunparse(self)


# Structured result objects for bytes data
class DefragResultBytes(_DefragResultBase, _ResultMixinBytes):
    __slots__ = ()

    def geturl(self):
        if self.fragment:
            return self.url + b'#' + self.fragment
        else:
            return self.url


class SplitResultBytes(_SplitResultBase, _NetlocResultMixinBytes):
    __slots__ = ()

    def geturl(self):
        return urlunsplit(self)


class ParseResultBytes(_ParseResultBase, _NetlocResultMixinBytes):
    __slots__ = ()

    def geturl(self):
        return urlunparse(self)


def _fix_result_transcoding():
    _result_pairs = (
        (DefragResult, DefragResultBytes),
        (SplitResult, SplitResultBytes),
        (ParseResult, ParseResultBytes),
    )
    for _decoded, _encoded in _result_pairs:
        _decoded._encoded_counterpart = _encoded
        _encoded._decoded_counterpart = _decoded


_fix_result_transcoding()
del _fix_result_transcoding


def urlparse(url, scheme='', allow_fragments=True):
    """Parse a URL into 6 components:
    <scheme>://<netloc>/<path>;<params>?<query>#<fragment>
    """
    url, scheme, _coerce_result = _coerce_args(url, scheme)
    splitresult = urlsplit(url, scheme, allow_fragments)
    scheme, netloc, url, query, fragment = splitresult
    if scheme in uses_params and ';' in url:
        url, params = _splitparams(url)
    else:
        params = ''
    result = ParseResult(scheme, netloc, url, params, query, fragment)
    return _coerce_result(result)


def _splitparams(url):
    if '/' in url:
        i = url.find(';', url.rfind('/'))
        if i < 0:
            return url, ''
    else:
        i = url.find(';')
    return url[:i], url[i + 1:]


def _splitnetloc(url, start=0):
    delim = len(url)
    for c in '/?#':
        wdelim = url.find(c, start)
        if wdelim >= 0:
            delim = min(delim, wdelim)
    return url[start:delim], url[delim:]


def _checknetloc(netloc):
    if not netloc or netloc.isascii():
        return
    for c in netloc:
        if c in _NFKC_INVALID_NETLOC_CHARS:
            raise ValueError("netloc '" + netloc + "' contains invalid " +
                             "characters under NFKC normalization")


def _is_valid_ipv4(addr):
    if not addr or not addr.isascii():
        return False
    octets = addr.split('.')
    if len(octets) != 4:
        return False
    for oct_str in octets:
        if not oct_str or len(oct_str) > 3:
            return False
        if len(oct_str) > 1 and oct_str[0] == '0':
            return False
        if not oct_str.isdigit():
            return False
        if int(oct_str) > 255:
            return False
    return True


def _is_valid_hextet(part):
    if not part or len(part) > 4:
        return False
    for c in part:
        if c not in _HEX_CHARS:
            return False
    return True


def _is_valid_ipv6(addr):
    if not addr:
        return False
    addr_part, has_pct, scope = addr.partition('%')
    if has_pct:
        if not scope or '%' in scope:
            return False
    if not addr_part or not addr_part.isascii():
        return False
    if ':::' in addr_part:
        return False
    double_colon_count = addr_part.count('::')
    if double_colon_count > 1:
        return False
    if double_colon_count == 1:
        left_s, right_s = addr_part.split('::')
        left = left_s.split(':') if left_s else []
        right = right_s.split(':') if right_s else []
        ipv4_groups = 0
        if right and '.' in right[-1]:
            if not _is_valid_ipv4(right[-1]):
                return False
            right = right[:-1]
            ipv4_groups = 2
        for part in left:
            if not _is_valid_hextet(part):
                return False
        for part in right:
            if not _is_valid_hextet(part):
                return False
        return len(left) + len(right) + ipv4_groups <= 7
    parts = addr_part.split(':')
    ipv4_groups = 0
    if '.' in parts[-1]:
        if not _is_valid_ipv4(parts[-1]):
            return False
        parts = parts[:-1]
        ipv4_groups = 2
    if len(parts) + ipv4_groups != 8:
        return False
    for part in parts:
        if not _is_valid_hextet(part):
            return False
    return True


def _check_bracketed_netloc(netloc):
    hostname_and_port = netloc.rpartition('@')[2]
    before_bracket, have_open_br, bracketed = hostname_and_port.partition('[')
    if have_open_br:
        if before_bracket:
            raise ValueError("Invalid IPv6 URL")
        hostname, _, port = bracketed.partition(']')
        if port and not port.startswith(":"):
            raise ValueError("Invalid IPv6 URL")
    else:
        hostname, _, port = hostname_and_port.partition(':')
    _check_bracketed_host(hostname)


def _check_bracketed_host(hostname):
    if hostname.startswith('v'):
        if not re.match(r"\Av[a-fA-F0-9]+\..+\Z", hostname):
            raise ValueError("IPvFuture address is invalid")
    else:
        if _is_valid_ipv4(hostname):
            raise ValueError("An IPv4 address cannot be in brackets")
        if not _is_valid_ipv6(hostname):
            raise ValueError(f"{hostname!r} does not appear to be an IPv4 or IPv6 address")


@functools.lru_cache(typed=True)
def urlsplit(url, scheme='', allow_fragments=True):
    """Parse a URL into 5 components:
    <scheme>://<netloc>/<path>?<query>#<fragment>
    """
    url, scheme, _coerce_result = _coerce_args(url, scheme)
    url = url.lstrip(_WHATWG_C0_CONTROL_OR_SPACE)
    scheme = scheme.strip(_WHATWG_C0_CONTROL_OR_SPACE)

    for b in _UNSAFE_URL_BYTES_TO_REMOVE:
        url = url.replace(b, "")
        scheme = scheme.replace(b, "")

    allow_fragments = bool(allow_fragments)
    netloc = query = fragment = ''
    i = url.find(':')
    if i > 0 and url[0].isascii() and url[0].isalpha():
        for c in url[:i]:
            if c not in scheme_chars:
                break
        else:
            scheme, url = url[:i].lower(), url[i + 1:]
    if url[:2] == '//':
        netloc, url = _splitnetloc(url, 2)
        if (('[' in netloc and ']' not in netloc) or
                (']' in netloc and '[' not in netloc)):
            raise ValueError("Invalid IPv6 URL")
        if '[' in netloc and ']' in netloc:
            _check_bracketed_netloc(netloc)
    if allow_fragments and '#' in url:
        url, fragment = url.split('#', 1)
    if '?' in url:
        url, query = url.split('?', 1)
    _checknetloc(netloc)
    v = SplitResult(scheme, netloc, url, query, fragment)
    return _coerce_result(v)


def urlunparse(components):
    """Put a parsed URL back together again."""
    scheme, netloc, url, params, query, fragment, _coerce_result = (
        _coerce_args(*components))
    if params:
        url = "%s;%s" % (url, params)
    return _coerce_result(urlunsplit((scheme, netloc, url, query, fragment)))


def urlunsplit(components):
    """Combine the elements of a tuple as returned by urlsplit() into a
    complete URL as a string."""
    scheme, netloc, url, query, fragment, _coerce_result = (
        _coerce_args(*components))
    if netloc:
        if url and url[:1] != '/':
            url = '/' + url
        url = '//' + netloc + url
    elif url[:2] == '//':
        url = '//' + url
    elif scheme and scheme in uses_netloc and (not url or url[:1] == '/'):
        url = '//' + url
    if scheme:
        url = scheme + ':' + url
    if query:
        url = url + '?' + query
    if fragment:
        url = url + '#' + fragment
    return _coerce_result(url)


def urljoin(base, url, allow_fragments=True):
    """Join a base URL and a possibly relative URL to form an absolute
    interpretation of the latter."""
    if not base:
        return url
    if not url:
        return base

    base, url, _coerce_result = _coerce_args(base, url)
    bscheme, bnetloc, bpath, bparams, bquery, bfragment = (
        urlparse(base, '', allow_fragments))
    scheme, netloc, path, params, query, fragment = (
        urlparse(url, bscheme, allow_fragments))

    if scheme != bscheme or scheme not in uses_relative:
        return _coerce_result(url)
    if scheme in uses_netloc:
        if netloc:
            return _coerce_result(urlunparse((scheme, netloc, path,
                                              params, query, fragment)))
        netloc = bnetloc

    if not path and not params:
        path = bpath
        params = bparams
        if not query:
            query = bquery
        return _coerce_result(urlunparse((scheme, netloc, path,
                                          params, query, fragment)))

    base_parts = bpath.split('/')
    if base_parts[-1] != '':
        del base_parts[-1]

    if path[:1] == '/':
        segments = path.split('/')
    else:
        segments = base_parts + path.split('/')
        segments[1:-1] = filter(None, segments[1:-1])

    resolved_path = []

    for seg in segments:
        if seg == '..':
            try:
                resolved_path.pop()
            except IndexError:
                pass
        elif seg == '.':
            continue
        else:
            resolved_path.append(seg)

    if segments[-1] in ('.', '..'):
        resolved_path.append('')

    return _coerce_result(urlunparse((scheme, netloc, '/'.join(
        resolved_path) or '/', params, query, fragment)))


def urldefrag(url):
    """Removes any existing fragment from URL."""
    url, _coerce_result = _coerce_args(url)
    if '#' in url:
        s, n, p, a, q, frag = urlparse(url)
        defrag = urlunparse((s, n, p, a, q, ''))
    else:
        frag = ''
        defrag = url
    return _coerce_result(DefragResult(defrag, frag))


_hexdig = '0123456789ABCDEFabcdef'
_hextobyte = None


def unquote_to_bytes(string):
    """unquote_to_bytes('abc%20def') -> b'abc def'."""
    return bytes(_unquote_impl(string))


def _unquote_impl(string):
    if not string:
        string.split
        return b''
    if isinstance(string, str):
        string = string.encode('utf-8')
    bits = string.split(b'%')
    if len(bits) == 1:
        return string
    res = bytearray(bits[0])
    append = res.extend
    global _hextobyte
    if _hextobyte is None:
        _hextobyte = {(a + b).encode(): bytes.fromhex(a + b)
                      for a in _hexdig for b in _hexdig}
    for item in bits[1:]:
        try:
            append(_hextobyte[item[:2]])
            append(item[2:])
        except KeyError:
            append(b'%')
            append(item)
    return res


def _generate_unquoted_parts(string, encoding, errors):
    n = len(string)
    i = 0
    while i < n:
        # Non-ASCII prefix
        start = i
        while i < n and ord(string[i]) >= 128:
            i += 1
        if i > start:
            yield string[start:i]
        # ASCII run
        start = i
        while i < n and ord(string[i]) < 128:
            i += 1
        if i > start:
            yield _unquote_impl(string[start:i]).decode(encoding, errors)


def unquote(string, encoding='utf-8', errors='replace'):
    """Replace %xx escapes by their single-character equivalent."""
    if isinstance(string, bytes):
        return _unquote_impl(string).decode(encoding, errors)
    if '%' not in string:
        string.split
        return string
    if encoding is None:
        encoding = 'utf-8'
    if errors is None:
        errors = 'replace'
    if string.isascii():
        return _unquote_impl(string).decode(encoding, errors)
    return ''.join(_generate_unquoted_parts(string, encoding, errors))


def parse_qs(qs, keep_blank_values=False, strict_parsing=False,
             encoding='utf-8', errors='replace', max_num_fields=None, separator='&'):
    """Parse a query given as a string argument."""
    parsed_result = {}
    pairs = parse_qsl(qs, keep_blank_values, strict_parsing,
                      encoding=encoding, errors=errors,
                      max_num_fields=max_num_fields, separator=separator)
    for name, value in pairs:
        if name in parsed_result:
            parsed_result[name].append(value)
        else:
            parsed_result[name] = [value]
    return parsed_result


def parse_qsl(qs, keep_blank_values=False, strict_parsing=False,
              encoding='utf-8', errors='replace', max_num_fields=None, separator='&'):
    """Parse a query given as a string argument."""
    if not separator or not isinstance(separator, (str, bytes)):
        raise ValueError("Separator must be of type string or bytes.")
    if isinstance(qs, str):
        if not isinstance(separator, str):
            separator = str(separator, 'ascii')
        eq = '='

        def _unquote(s):
            return unquote_plus(s, encoding=encoding, errors=errors)
    else:
        if not qs:
            return []
        if isinstance(qs, (bytes, bytearray)):
            qs = bytes(qs)
        else:
            raise TypeError(
                "memoryview: a bytes-like object is required, not %r" % type(qs).__name__
            )
        if isinstance(separator, str):
            separator = bytes(separator, 'ascii')
        eq = b'='

        def _unquote(s):
            return unquote_to_bytes(s.replace(b'+', b' '))

    if not qs:
        return []

    if max_num_fields is not None:
        num_fields = 1 + qs.count(separator)
        if max_num_fields < num_fields:
            raise ValueError('Max number of fields exceeded')

    r = []
    for name_value in qs.split(separator):
        if name_value or strict_parsing:
            name, has_eq, value = name_value.partition(eq)
            if not has_eq and strict_parsing:
                raise ValueError("bad query field: %r" % (name_value,))
            if value or keep_blank_values:
                name = _unquote(name)
                value = _unquote(value)
                r.append((name, value))
    return r


def unquote_plus(string, encoding='utf-8', errors='replace'):
    """Like unquote(), but also replace plus signs by spaces."""
    string = string.replace('+', ' ')
    return unquote(string, encoding, errors)


_ALWAYS_SAFE = frozenset(b'ABCDEFGHIJKLMNOPQRSTUVWXYZ'
                         b'abcdefghijklmnopqrstuvwxyz'
                         b'0123456789'
                         b'_.-~')
_ALWAYS_SAFE_BYTES = bytes(_ALWAYS_SAFE)


class _Quoter(dict):
    """A mapping from bytes numbers (in range(0,256)) to strings."""

    def __init__(self, safe):
        self.safe = _ALWAYS_SAFE | frozenset(safe)

    def __repr__(self):
        return f"<Quoter {dict(self)!r}>"

    def __missing__(self, b):
        res = chr(b) if b in self.safe else '%{:02X}'.format(b)
        self[b] = res
        return res


def quote(string, safe='/', encoding=None, errors=None):
    """quote('abc def') -> 'abc%20def'."""
    if isinstance(string, str):
        if not string:
            return string
        if encoding is None:
            encoding = 'utf-8'
        if errors is None:
            errors = 'strict'
        string = string.encode(encoding, errors)
    else:
        if encoding is not None:
            raise TypeError("quote() doesn't support 'encoding' for bytes")
        if errors is not None:
            raise TypeError("quote() doesn't support 'errors' for bytes")
    return quote_from_bytes(string, safe)


def quote_plus(string, safe='', encoding=None, errors=None):
    """Like quote(), but also replace ' ' with '+'."""
    if ((isinstance(string, str) and ' ' not in string) or
            (isinstance(string, bytes) and b' ' not in string)):
        return quote(string, safe, encoding, errors)
    if isinstance(safe, str):
        space = ' '
    else:
        space = b' '
    string = quote(string, safe + space, encoding, errors)
    return string.replace(' ', '+')


@functools.lru_cache
def _byte_quoter_factory(safe):
    safe_set = _ALWAYS_SAFE | frozenset(safe)
    table = tuple(chr(b) if b in safe_set else '%{:02X}'.format(b) for b in range(256))
    return table.__getitem__


def quote_from_bytes(bs, safe='/'):
    """Like quote(), but accepts a bytes object rather than a str."""
    if not isinstance(bs, (bytes, bytearray)):
        raise TypeError("quote_from_bytes() expected bytes")
    if not bs:
        return ''
    if isinstance(safe, str):
        safe = safe.encode('ascii', 'ignore')
    else:
        safe = bytes([c for c in safe if c < 128])
    if not bs.rstrip(_ALWAYS_SAFE_BYTES + safe):
        return bs.decode()
    quoter = _byte_quoter_factory(safe)
    if (bs_len := len(bs)) < 200_000:
        return ''.join(map(quoter, bs))
    else:
        chunk_size = math.isqrt(bs_len)
        chunks = [''.join(map(quoter, bs[i:i + chunk_size]))
                  for i in range(0, bs_len, chunk_size)]
        return ''.join(chunks)


def urlencode(query, doseq=False, safe='', encoding=None, errors=None,
              quote_via=quote_plus):
    """Encode a dict or sequence of two-element tuples into a URL query string."""
    if hasattr(query, "items"):
        query = query.items()
    else:
        try:
            if len(query) and not isinstance(query[0], tuple):
                raise TypeError
        except TypeError as err:
            raise TypeError("not a valid non-string sequence "
                            "or mapping object") from err

    l = []
    if not doseq:
        for k, v in query:
            if isinstance(k, bytes):
                k = quote_via(k, safe)
            else:
                k = quote_via(str(k), safe, encoding, errors)

            if isinstance(v, bytes):
                v = quote_via(v, safe)
            else:
                v = quote_via(str(v), safe, encoding, errors)
            l.append(k + '=' + v)
    else:
        for k, v in query:
            if isinstance(k, bytes):
                k = quote_via(k, safe)
            else:
                k = quote_via(str(k), safe, encoding, errors)

            if isinstance(v, bytes):
                v = quote_via(v, safe)
                l.append(k + '=' + v)
            elif isinstance(v, str):
                v = quote_via(v, safe, encoding, errors)
                l.append(k + '=' + v)
            else:
                try:
                    x = len(v)
                except TypeError:
                    v = quote_via(str(v), safe, encoding, errors)
                    l.append(k + '=' + v)
                else:
                    for elt in v:
                        if isinstance(elt, bytes):
                            elt = quote_via(elt, safe)
                        else:
                            elt = quote_via(str(elt), safe, encoding, errors)
                        l.append(k + '=' + elt)
    return '&'.join(l)


def to_bytes(url):
    warnings.warn("urllib.parse.to_bytes() is deprecated as of 3.8",
                  DeprecationWarning, stacklevel=2)
    return _to_bytes(url)


def _to_bytes(url):
    if isinstance(url, str):
        try:
            url = url.encode("ASCII").decode()
        except UnicodeError:
            raise UnicodeError("URL " + repr(url) +
                               " contains non-ASCII characters")
    return url


def unwrap(url):
    """Transform a string like '<URL:scheme://host/path>' into 'scheme://host/path'."""
    url = str(url).strip()
    if url[:1] == '<' and url[-1:] == '>':
        url = url[1:-1].strip()
    if url[:4] == 'URL:':
        url = url[4:].strip()
    return url


def splittype(url):
    warnings.warn("urllib.parse.splittype() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splittype(url)


_typeprog = None


def _splittype(url):
    global _typeprog
    if _typeprog is None:
        _typeprog = re.compile('([^/:]+):(.*)', re.DOTALL)

    match = _typeprog.match(url)
    if match:
        scheme, data = match.groups()
        return scheme.lower(), data
    return None, url


def splithost(url):
    warnings.warn("urllib.parse.splithost() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splithost(url)


_hostprog = None


def _splithost(url):
    global _hostprog
    if _hostprog is None:
        _hostprog = re.compile('//([^/#?]*)(.*)', re.DOTALL)

    match = _hostprog.match(url)
    if match:
        host_port, path = match.groups()
        if path and path[0] != '/':
            path = '/' + path
        return host_port, path
    return None, url


def splituser(host):
    warnings.warn("urllib.parse.splituser() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splituser(host)


def _splituser(host):
    user, delim, host = host.rpartition('@')
    return (user if delim else None), host


def splitpasswd(user):
    warnings.warn("urllib.parse.splitpasswd() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitpasswd(user)


def _splitpasswd(user):
    user, delim, passwd = user.partition(':')
    return user, (passwd if delim else None)


def splitport(host):
    warnings.warn("urllib.parse.splitport() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitport(host)


_portprog = None


def _splitport(host):
    global _portprog
    if _portprog is None:
        _portprog = re.compile('(.*):([0-9]*)', re.DOTALL)

    match = _portprog.fullmatch(host)
    if match:
        host, port = match.groups()
        if port:
            return host, port
    return host, None


def splitnport(host, defport=-1):
    warnings.warn("urllib.parse.splitnport() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitnport(host, defport)


def _splitnport(host, defport=-1):
    host, delim, port = host.rpartition(':')
    if not delim:
        host = port
    elif port:
        if port.isdigit() and port.isascii():
            nport = int(port)
        else:
            nport = None
        return host, nport
    return host, defport


def splitquery(url):
    warnings.warn("urllib.parse.splitquery() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitquery(url)


def _splitquery(url):
    path, delim, query = url.rpartition('?')
    if delim:
        return path, query
    return url, None


def splittag(url):
    warnings.warn("urllib.parse.splittag() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splittag(url)


def _splittag(url):
    path, delim, tag = url.rpartition('#')
    if delim:
        return path, tag
    return url, None


def splitattr(url):
    warnings.warn("urllib.parse.splitattr() is deprecated as of 3.8, "
                  "use urllib.parse.urlparse() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitattr(url)


def _splitattr(url):
    words = url.split(';')
    return words[0], words[1:]


def splitvalue(attr):
    warnings.warn("urllib.parse.splitvalue() is deprecated as of 3.8, "
                  "use urllib.parse.parse_qsl() instead",
                  DeprecationWarning, stacklevel=2)
    return _splitvalue(attr)


def _splitvalue(attr):
    attr, delim, value = attr.partition('=')
    return attr, (value if delim else None)
