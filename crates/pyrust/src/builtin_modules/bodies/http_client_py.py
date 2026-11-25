r"""HTTP/1.1 client library."""

import collections.abc
import errno
import io
import re
import socket
import sys
from urllib.parse import urlsplit

__all__ = [
    "HTTPResponse", "HTTPConnection",
    "HTTPException", "NotConnected", "UnknownProtocol",
    "UnknownTransferEncoding", "UnimplementedFileMode",
    "IncompleteRead", "InvalidURL", "ImproperConnectionState",
    "CannotSendRequest", "CannotSendHeader", "ResponseNotReady",
    "BadStatusLine", "LineTooLong", "RemoteDisconnected", "error",
    "responses",
]

HTTP_PORT = 80
HTTPS_PORT = 443

_UNKNOWN = 'UNKNOWN'

# connection states
_CS_IDLE = 'Idle'
_CS_REQ_STARTED = 'Request-started'
_CS_REQ_SENT = 'Request-sent'

# maximal line length when calling readline().
_MAXLINE = 65536
_MAXHEADERS = 100

# Data larger than this will be read in chunks, to prevent extreme
# overallocation.
_MIN_READ_BUF_SIZE = 1 << 20

_is_legal_header_name = re.compile(rb'[^:\s][^:\r\n]*').fullmatch
_is_illegal_header_value = re.compile(rb'\n(?![ \t])|\r(?![ \t\n])').search

_contains_disallowed_url_pchar_re = re.compile('[\x00-\x20\x7f]')
_contains_disallowed_method_pchar_re = re.compile('[\x00-\x1f]')

_METHODS_EXPECTING_BODY = {'PATCH', 'POST', 'PUT'}


def _encode(data, name='data'):
    """Call data.encode("latin-1") but show a better error message."""
    try:
        return data.encode("latin-1")
    except UnicodeEncodeError as err:
        raise UnicodeEncodeError(
            err.encoding,
            err.object,
            err.start,
            err.end,
            "%s (%.20r) is not valid Latin-1. Use %s.encode('utf-8') "
            "if you want to send it encoded in UTF-8." % (
                name.title(), data[err.start:err.end], name
            ),
        ) from None


def _encode_host(host):
    try:
        return host.encode("ascii")
    except UnicodeEncodeError:
        return host.encode("utf-8")


def _strip_ipv6_iface(enc_name):
    """Remove interface scope from IPv6 address."""
    enc_name, percent, _ = enc_name.partition(b"%")
    if percent:
        assert enc_name.startswith(b'['), enc_name
        enc_name += b']'
    return enc_name


def _unquote_param(val):
    if len(val) >= 2:
        if val.startswith('"') and val.endswith('"'):
            return val[1:-1].replace('\\\\', '\\').replace('\\"', '"')
        if val.startswith('<') and val.endswith('>'):
            return val[1:-1]
    return val


class HTTPMessage:
    """Case-insensitive HTTP header container compatible with email.message.Message."""

    def __init__(self):
        self._headers = []
        self._default_type = 'text/plain'

    def __len__(self):
        return len(self._headers)

    def __iter__(self):
        for k, _ in self._headers:
            yield k

    def __contains__(self, name):
        lower = name.lower()
        return any(k.lower() == lower for k, _ in self._headers)

    def __getitem__(self, name):
        return self.get(name)

    def __setitem__(self, name, val):
        self._headers.append((name, val))

    def __delitem__(self, name):
        lower = name.lower()
        self._headers = [(k, v) for k, v in self._headers if k.lower() != lower]

    def keys(self):
        return [k for k, _ in self._headers]

    def values(self):
        return [v for _, v in self._headers]

    def items(self):
        return list(self._headers)

    def get(self, name, failobj=None):
        lower = name.lower()
        for k, v in self._headers:
            if k.lower() == lower:
                return v
        return failobj

    def get_all(self, name, failobj=None):
        lower = name.lower()
        values = [v for k, v in self._headers if k.lower() == lower]
        if not values:
            return failobj
        return values

    def replace_header(self, _name, _value):
        lower = _name.lower()
        for i, (k, _) in enumerate(self._headers):
            if k.lower() == lower:
                self._headers[i] = (_name, _value)
                return
        raise KeyError(_name)

    def add_header(self, _name, _value, **_params):
        parts = []
        if _value is not None:
            parts.append(_value)
        for k, v in _params.items():
            k = k.replace('_', '-')
            if v is None:
                parts.append(k)
            else:
                parts.append(f'{k}="{v}"')
        self._headers.append((_name, '; '.join(parts)))

    def set_default_type(self, ctype):
        self._default_type = ctype

    def get_default_type(self):
        return self._default_type

    def _split_params(self, header='content-type'):
        value = self.get(header)
        if value is None:
            return None
        parts = value.split(';')
        result = [(parts[0].strip().lower(), '')]
        for part in parts[1:]:
            part = part.strip()
            if not part:
                continue
            if '=' in part:
                k, v = part.split('=', 1)
                result.append((k.strip().lower(), v.strip()))
            else:
                result.append((part.lower(), ''))
        return result

    def get_content_type(self):
        params = self._split_params('content-type')
        if params is None:
            return self._default_type
        ctype = params[0][0]
        if ctype.count('/') != 1:
            return 'text/plain'
        return ctype

    def get_content_maintype(self):
        return self.get_content_type().split('/', 1)[0]

    def get_content_subtype(self):
        return self.get_content_type().split('/', 1)[1]

    def get_param(self, param, failobj=None, header='content-type', unquote=True):
        params = self._split_params(header)
        if params is None:
            return failobj
        target = param.lower()
        for k, v in params:
            if k == target:
                if unquote and isinstance(v, str):
                    return _unquote_param(v)
                return v
        return failobj

    def get_content_charset(self, failobj=None):
        charset = self.get_param('charset', None)
        if charset is None:
            return failobj
        return charset.lower()

    def getallmatchingheaders(self, name):
        """Find all header lines matching a given header name."""
        name = name.lower() + ':'
        n = len(name)
        lst = []
        hit = 0
        for line in self.keys():
            if line[:n].lower() == name:
                hit = 1
            elif not line[:1].isspace():
                hit = 0
            if hit:
                lst.append(line)
        return lst

    def as_string(self):
        lines = []
        for k, v in self._headers:
            clean_v = str(v).replace('\r\n', '\n')
            lines.append(f"{k}: {clean_v}\n")
        lines.append("\n")
        return "".join(lines)

    __str__ = as_string


# Publish HTTPMessage onto sys.modules["http.client"] before importing `http`,
# so sibling `http.server` stitched by `http` can reference it cleanly if
# `import http.client` was the first import.
_self_mod = sys.modules.get("http.client")
if _self_mod is not None:
    _self_mod.HTTPMessage = HTTPMessage

import http

# Mirror HTTPStatus members onto http.client for backwards compatibility
for _status in http.HTTPStatus:
    globals()[_status.name] = _status

# Mapping status codes to official W3C phrases
responses = {v: v.phrase for v in http.HTTPStatus}


def _read_headers(fp):
    """Reads potential header lines into a list from a file pointer.

    Length of line is limited by _MAXLINE, and number of
    headers is limited by _MAXHEADERS.
    """
    headers = []
    while True:
        line = fp.readline(_MAXLINE + 1)
        if len(line) > _MAXLINE:
            raise LineTooLong("header line")
        headers.append(line)
        if len(headers) > _MAXHEADERS:
            raise HTTPException("got more than %d headers" % _MAXHEADERS)
        if line in (b'\r\n', b'\n', b''):
            break
    return headers


def _parse_header_lines(header_lines, _class=HTTPMessage):
    """Parse RFC 2822 header lines into an HTTPMessage (or custom _class)."""
    msg = _class()
    last_name = None
    last_value = None
    last_sep = None

    def flush():
        nonlocal last_name, last_value, last_sep
        if last_name is not None:
            msg[last_name] = last_value
            last_name = None
            last_value = None
            last_sep = None

    for raw in header_lines:
        if raw in (b'\r\n', b'\n', b''):
            break
        line = raw.decode('iso-8859-1')
        if line.endswith('\r\n'):
            line_sep = '\r\n'
            content = line[:-2]
        elif line.endswith('\n'):
            line_sep = '\n'
            content = line[:-1]
        else:
            line_sep = '\n'
            content = line

        if content and content[0] in (' ', '\t'):
            if last_name is not None:
                last_value = last_value + (last_sep or '\r\n') + content
                last_sep = line_sep
            continue

        colon = content.find(':')
        if colon < 0:
            flush()
            break

        flush()
        last_name = content[:colon]
        last_value = content[colon + 1:].lstrip(' \t')
        last_sep = line_sep

    flush()
    return msg


def parse_headers(fp, _class=HTTPMessage):
    """Parses only RFC2822 headers from a file pointer."""
    headers = _read_headers(fp)
    return _parse_header_lines(headers, _class)


class HTTPResponse(io.BufferedIOBase):

    def __init__(self, sock, debuglevel=0, method=None, url=None):
        self.fp = sock.makefile("rb")
        self.debuglevel = debuglevel
        self._method = method
        self.url = url
        self._closed = False

        self.headers = self.msg = None

        # from the Status-Line of the response
        self.version = _UNKNOWN
        self.status = _UNKNOWN
        self.reason = _UNKNOWN

        self.chunked = _UNKNOWN
        self.chunk_left = _UNKNOWN
        self.length = _UNKNOWN
        self.will_close = _UNKNOWN

    @property
    def closed(self):
        return self._closed

    def _read_status(self):
        line = str(self.fp.readline(_MAXLINE + 1), "iso-8859-1")
        if len(line) > _MAXLINE:
            raise LineTooLong("status line")
        if self.debuglevel > 0:
            print("reply:", repr(line))
        if not line:
            raise RemoteDisconnected(
                "Remote end closed connection without response"
            )
        try:
            version, status, reason = line.split(None, 2)
        except ValueError:
            try:
                version, status = line.split(None, 1)
                reason = ""
            except ValueError:
                version = ""
        if not version.startswith("HTTP/"):
            self._close_conn()
            raise BadStatusLine(line)

        try:
            status = int(status)
            if status < 100 or status > 999:
                raise BadStatusLine(line)
        except ValueError:
            raise BadStatusLine(line)
        return version, status, reason

    def begin(self):
        if self.headers is not None:
            return

        # read until we get a non-100 response
        while True:
            version, status, reason = self._read_status()
            if status != CONTINUE:
                break
            skipped_headers = _read_headers(self.fp)
            if self.debuglevel > 0:
                print("headers:", skipped_headers)
            del skipped_headers

        self.code = self.status = status
        self.reason = reason.strip()
        if version in ("HTTP/1.0", "HTTP/0.9"):
            self.version = 10
        elif version.startswith("HTTP/1."):
            self.version = 11
        else:
            raise UnknownProtocol(version)

        self.headers = self.msg = parse_headers(self.fp)

        if self.debuglevel > 0:
            for hdr, val in self.headers.items():
                print("header:", hdr + ":", val)

        tr_enc = self.headers.get("transfer-encoding")
        if tr_enc and tr_enc.lower() == "chunked":
            self.chunked = True
            self.chunk_left = None
        else:
            self.chunked = False

        self.will_close = self._check_close()

        self.length = None
        length = self.headers.get("content-length")
        if length and not self.chunked:
            try:
                self.length = int(length)
            except ValueError:
                self.length = None
            else:
                if self.length < 0:
                    self.length = None
        else:
            self.length = None

        if (
            status == NO_CONTENT
            or status == NOT_MODIFIED
            or 100 <= status < 200
            or self._method == "HEAD"
        ):
            self.length = 0

        if not self.will_close and not self.chunked and self.length is None:
            self.will_close = True

    def _check_close(self):
        conn = self.headers.get("connection")
        if self.version == 11:
            if conn and "close" in conn.lower():
                return True
            return False

        if self.headers.get("keep-alive"):
            return False

        if conn and "keep-alive" in conn.lower():
            return False

        pconn = self.headers.get("proxy-connection")
        if pconn and "keep-alive" in pconn.lower():
            return False

        return True

    def _close_conn(self):
        fp = self.fp
        self.fp = None
        fp.close()

    def close(self):
        self._closed = True
        if self.fp:
            self._close_conn()

    def flush(self):
        if self._closed:
            raise ValueError("I/O operation on closed file.")
        if self.fp:
            self.fp.flush()

    def readable(self):
        return True

    def writable(self):
        return False

    def seekable(self):
        return False

    def isclosed(self):
        """True if the connection is closed."""
        return self.fp is None

    def read(self, amt=None):
        """Read and return the response body, or up to the next amt bytes."""
        if self.fp is None:
            return b""

        if self._method == "HEAD":
            self._close_conn()
            return b""

        if self.chunked:
            return self._read_chunked(amt)

        if amt is not None and amt >= 0:
            if self.length is not None and amt > self.length:
                amt = self.length
            s = self.fp.read(amt)
            if not s and amt:
                self._close_conn()
            elif self.length is not None:
                self.length -= len(s)
                if not self.length:
                    self._close_conn()
            return s
        else:
            if self.length is None:
                s = self.fp.read()
            else:
                try:
                    s = self._safe_read(self.length)
                except IncompleteRead:
                    self._close_conn()
                    raise
                self.length = 0
            self._close_conn()
            return s

    def readinto(self, b):
        """Read up to len(b) bytes into bytearray b and return the number
        of bytes read.
        """
        if self.fp is None:
            return 0

        if self._method == "HEAD":
            self._close_conn()
            return 0

        if self.chunked:
            return self._readinto_chunked(b)

        if self.length is not None:
            if len(b) > self.length:
                b = memoryview(b)[0:self.length]

        n = self.fp.readinto(b)
        if not n and b:
            self._close_conn()
        elif self.length is not None:
            self.length -= n
            if not self.length:
                self._close_conn()
        return n

    def _read_next_chunk_size(self):
        line = self.fp.readline(_MAXLINE + 1)
        if len(line) > _MAXLINE:
            raise LineTooLong("chunk size")
        i = line.find(b";")
        if i >= 0:
            line = line[:i]
        try:
            return int(line, 16)
        except ValueError:
            self._close_conn()
            raise

    def _read_and_discard_trailer(self):
        while True:
            line = self.fp.readline(_MAXLINE + 1)
            if len(line) > _MAXLINE:
                raise LineTooLong("trailer line")
            if not line:
                break
            if line in (b'\r\n', b'\n', b''):
                break

    def _get_chunk_left(self):
        chunk_left = self.chunk_left
        if not chunk_left:
            if chunk_left is not None:
                self._safe_read(2)
            try:
                chunk_left = self._read_next_chunk_size()
            except ValueError:
                raise IncompleteRead(b'')
            if chunk_left == 0:
                self._read_and_discard_trailer()
                self._close_conn()
                chunk_left = None
            self.chunk_left = chunk_left
        return chunk_left

    def _read_chunked(self, amt=None):
        assert self.chunked != _UNKNOWN
        if amt is not None and amt < 0:
            amt = None
        value = []
        try:
            while (chunk_left := self._get_chunk_left()) is not None:
                if amt is not None and amt <= chunk_left:
                    value.append(self._safe_read(amt))
                    self.chunk_left = chunk_left - amt
                    break

                value.append(self._safe_read(chunk_left))
                if amt is not None:
                    amt -= chunk_left
                self.chunk_left = 0
            return b''.join(value)
        except IncompleteRead as exc:
            raise IncompleteRead(b''.join(value)) from exc

    def _readinto_chunked(self, b):
        assert self.chunked != _UNKNOWN
        total_bytes = 0
        mvb = memoryview(b)
        try:
            while True:
                chunk_left = self._get_chunk_left()
                if chunk_left is None:
                    return total_bytes

                if len(mvb) <= chunk_left:
                    n = self._safe_readinto(mvb)
                    self.chunk_left = chunk_left - n
                    return total_bytes + n

                temp_mvb = mvb[:chunk_left]
                n = self._safe_readinto(temp_mvb)
                mvb = mvb[n:]
                total_bytes += n
                self.chunk_left = 0

        except IncompleteRead:
            raise IncompleteRead(bytes(b[0:total_bytes]))

    def _safe_read(self, amt):
        cursize = min(amt, _MIN_READ_BUF_SIZE)
        data = self.fp.read(cursize)
        if len(data) >= amt:
            return data
        if len(data) < cursize:
            raise IncompleteRead(data, amt - len(data))

        buf = io.BytesIO(data)
        buf.seek(0, 2)
        while True:
            delta = min(cursize, amt - cursize)
            buf.write(self.fp.read(delta))
            if buf.tell() >= amt:
                return buf.getvalue()
            cursize += delta
            if buf.tell() < cursize:
                raise IncompleteRead(buf.getvalue(), amt - buf.tell())

    def _safe_readinto(self, b):
        amt = len(b)
        n = self.fp.readinto(b)
        if n < amt:
            raise IncompleteRead(bytes(b[:n]), amt - n)
        return n

    def read1(self, n=-1):
        if self.fp is None or self._method == "HEAD":
            return b""
        if self.chunked:
            return self._read1_chunked(n)
        if self.length is not None and (n < 0 or n > self.length):
            n = self.length
        result = self.fp.read1(n)
        if not result and n:
            self._close_conn()
        elif self.length is not None:
            self.length -= len(result)
            if not self.length:
                self._close_conn()
        return result

    def _readline_chunked(self, limit=-1):
        out = bytearray()
        while limit < 0 or len(out) < limit:
            ch = self._read_chunked(1)
            if not ch:
                break
            out.extend(ch)
            if ch == b"\n":
                break
        return bytes(out)

    def readline(self, limit=-1):
        if self.fp is None or self._method == "HEAD":
            return b""
        if limit is None:
            limit = -1
        if self.chunked:
            return self._readline_chunked(limit)
        if self.length is not None and (limit < 0 or limit > self.length):
            limit = self.length
        result = self.fp.readline(limit)
        if not result and limit:
            self._close_conn()
        elif self.length is not None:
            self.length -= len(result)
            if not self.length:
                self._close_conn()
        return result

    def readlines(self, hint=-1):
        lines = []
        total = 0
        while True:
            line = self.readline()
            if not line:
                break
            lines.append(line)
            total += len(line)
            if hint is not None and hint > 0 and total >= hint:
                break
        return lines

    def _read1_chunked(self, n):
        chunk_left = self._get_chunk_left()
        if chunk_left is None or n == 0:
            return b''
        if not (0 <= n <= chunk_left):
            n = chunk_left
        read = self.fp.read1(n)
        self.chunk_left -= len(read)
        if not read:
            raise IncompleteRead(b"")
        return read

    def fileno(self):
        return self.fp.fileno()

    def getheader(self, name, default=None):
        if self.headers is None:
            raise ResponseNotReady()
        headers = self.headers.get_all(name) or default
        if isinstance(headers, str) or not hasattr(headers, '__iter__'):
            return headers
        else:
            return ', '.join(headers)

    def getheaders(self):
        """Return list of (header, value) tuples."""
        if self.headers is None:
            raise ResponseNotReady()
        return list(self.headers.items())

    def __iter__(self):
        return self

    def __next__(self):
        line = self.readline()
        if not line:
            raise StopIteration
        return line

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
        return False

    def info(self):
        return self.headers

    def geturl(self):
        return self.url

    def getcode(self):
        return self.status


class HTTPConnection:

    _http_vsn = 11
    _http_vsn_str = 'HTTP/1.1'

    response_class = HTTPResponse
    default_port = HTTP_PORT
    auto_open = 1
    debuglevel = 0

    @staticmethod
    def _is_textIO(stream):
        return isinstance(stream, io.TextIOBase)

    @staticmethod
    def _get_content_length(body, method):
        if body is None:
            if method.upper() in _METHODS_EXPECTING_BODY:
                return 0
            else:
                return None

        if hasattr(body, 'read'):
            return None

        try:
            mv = memoryview(body)
            return mv.nbytes
        except TypeError:
            pass

        if isinstance(body, str):
            return len(body)

        return None

    def __init__(
        self,
        host,
        port=None,
        timeout=socket._GLOBAL_DEFAULT_TIMEOUT,
        source_address=None,
        blocksize=8192,
    ):
        self.timeout = timeout
        self.source_address = source_address
        self.blocksize = blocksize
        self.sock = None
        self._buffer = []
        self.__response = None
        self.__state = _CS_IDLE
        self._method = None
        self._tunnel_host = None
        self._tunnel_port = None
        self._tunnel_headers = {}
        self._raw_proxy_headers = None

        (self.host, self.port) = self._get_hostport(host, port)
        self._validate_host(self.host)

        self._create_connection = socket.create_connection

    def set_tunnel(self, host, port=None, headers=None):
        if self.sock:
            raise RuntimeError("Can't set up tunnel for established connection")

        self._tunnel_host, self._tunnel_port = self._get_hostport(host, port)
        if headers:
            self._tunnel_headers = headers.copy()
        else:
            self._tunnel_headers.clear()

        if not any(header.lower() == "host" for header in self._tunnel_headers):
            encoded_host = _encode_host(self._tunnel_host).decode("ascii", "replace")
            self._tunnel_headers["Host"] = "%s:%d" % (
                encoded_host, self._tunnel_port
            )

    def _get_hostport(self, host, port):
        if port is None:
            i = host.rfind(':')
            j = host.rfind(']')
            if i > j:
                try:
                    port = int(host[i + 1:])
                except ValueError:
                    if host[i + 1:] == "":
                        port = self.default_port
                    else:
                        raise InvalidURL("nonnumeric port: '%s'" % host[i + 1:])
                host = host[:i]
            else:
                port = self.default_port
        if host and host[0] == '[' and host[-1] == ']':
            host = host[1:-1]

        return (host, port)

    def set_debuglevel(self, level):
        self.debuglevel = level

    def _wrap_ipv6(self, ip):
        if b':' in ip and ip[0] != b'['[0]:
            return b"[" + ip + b"]"
        return ip

    def _tunnel(self):
        connect = b"CONNECT %s:%d %s\r\n" % (
            self._wrap_ipv6(_encode_host(self._tunnel_host)),
            self._tunnel_port,
            self._http_vsn_str.encode("ascii"),
        )
        headers = [connect]
        for header, value in self._tunnel_headers.items():
            headers.append(f"{header}: {value}\r\n".encode("latin-1"))
        headers.append(b"\r\n")
        self.send(b"".join(headers))
        del headers

        response = self.response_class(self.sock, method=self._method)
        try:
            (version, code, message) = response._read_status()
            self._raw_proxy_headers = _read_headers(response.fp)

            if self.debuglevel > 0:
                for header in self._raw_proxy_headers:
                    print('header:', header.decode())

            if code != http.HTTPStatus.OK:
                self.close()
                raise OSError(
                    f"Tunnel connection failed: {code} {message.strip()}"
                )
        finally:
            response.close()

    def get_proxy_response_headers(self):
        return (
            _parse_header_lines(self._raw_proxy_headers)
            if self._raw_proxy_headers is not None
            else None
        )

    def connect(self):
        """Connect to the host and port specified in __init__."""
        sys.audit("http.client.connect", self, self.host, self.port)
        self.sock = self._create_connection(
            (self.host, self.port), self.timeout, self.source_address
        )
        try:
            self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except OSError as e:
            if e.errno != errno.ENOPROTOOPT:
                raise

        if self._tunnel_host:
            self._tunnel()

    def close(self):
        """Close the connection to the HTTP server."""
        self.__state = _CS_IDLE
        try:
            sock = self.sock
            if sock:
                self.sock = None
                sock.close()
        finally:
            response = self.__response
            if response:
                self.__response = None
                response.close()

    def send(self, data):
        """Send `data' to the server."""
        if self.sock is None:
            if self.auto_open:
                self.connect()
            else:
                raise NotConnected()

        if self.debuglevel > 0:
            print("send:", repr(data))
        if hasattr(data, "read"):
            if self.debuglevel > 0:
                print("sending a readable")
            encode = self._is_textIO(data)
            if encode and self.debuglevel > 0:
                print("encoding file using iso-8859-1")
            while datablock := data.read(self.blocksize):
                if encode:
                    datablock = datablock.encode("iso-8859-1")
                sys.audit("http.client.send", self, datablock)
                self.sock.sendall(datablock)
            return
        sys.audit("http.client.send", self, data)
        try:
            self.sock.sendall(data)
        except TypeError:
            if isinstance(data, collections.abc.Iterable):
                for d in data:
                    self.sock.sendall(d)
            else:
                raise TypeError(
                    "data should be a bytes-like object "
                    "or an iterable, got %r" % type(data)
                )

    def _output(self, s):
        self._buffer.append(s)

    def _read_readable(self, readable):
        if self.debuglevel > 0:
            print("reading a readable")
        encode = self._is_textIO(readable)
        if encode and self.debuglevel > 0:
            print("encoding file using iso-8859-1")
        while datablock := readable.read(self.blocksize):
            if encode:
                datablock = datablock.encode("iso-8859-1")
            yield datablock

    def _send_output(self, message_body=None, encode_chunked=False):
        self._buffer.extend((b"", b""))
        msg = b"\r\n".join(self._buffer)
        del self._buffer[:]
        self.send(msg)

        if message_body is not None:
            if hasattr(message_body, 'read'):
                chunks = self._read_readable(message_body)
            else:
                try:
                    memoryview(message_body)
                except TypeError:
                    try:
                        chunks = iter(message_body)
                    except TypeError:
                        raise TypeError(
                            "message_body should be a bytes-like "
                            "object or an iterable, got %r"
                            % type(message_body)
                        )
                else:
                    chunks = (message_body,)

            for chunk in chunks:
                if not chunk:
                    if self.debuglevel > 0:
                        print('Zero length chunk ignored')
                    continue

                if encode_chunked and self._http_vsn == 11:
                    chunk = (
                        f'{len(chunk):X}\r\n'.encode('ascii') + chunk + b'\r\n'
                    )
                self.send(chunk)

            if encode_chunked and self._http_vsn == 11:
                self.send(b'0\r\n\r\n')

    def putrequest(
        self, method, url, skip_host=False, skip_accept_encoding=False
    ):
        if self.__response and self.__response.isclosed():
            self.__response = None

        if self.__state == _CS_IDLE:
            self.__state = _CS_REQ_STARTED
        else:
            raise CannotSendRequest(self.__state)

        self._validate_method(method)
        self._method = method

        url = url or '/'
        self._validate_path(url)

        request = '%s %s %s' % (method, url, self._http_vsn_str)
        self._output(self._encode_request(request))

        if self._http_vsn == 11:
            if not skip_host:
                netloc = ''
                if url.startswith('http'):
                    _, netloc, _, _, _ = urlsplit(url)

                if netloc:
                    netloc_enc = _encode_host(netloc)
                    self.putheader('Host', _strip_ipv6_iface(netloc_enc))
                else:
                    if self._tunnel_host:
                        host = self._tunnel_host
                        port = self._tunnel_port
                    else:
                        host = self.host
                        port = self.port

                    host_enc = _encode_host(host)
                    host_enc = self._wrap_ipv6(host_enc)
                    if ":" in host:
                        host_enc = _strip_ipv6_iface(host_enc)

                    if port == self.default_port:
                        self.putheader('Host', host_enc)
                    else:
                        host_str = host_enc.decode("ascii")
                        self.putheader('Host', "%s:%s" % (host_str, port))

            if not skip_accept_encoding:
                self.putheader('Accept-Encoding', 'identity')

    def _encode_request(self, request):
        return request.encode('ascii')

    def _validate_method(self, method):
        match = _contains_disallowed_method_pchar_re.search(method)
        if match:
            raise ValueError(
                f"method can't contain control characters. {method!r} "
                f"(found at least {match.group()!r})"
            )

    def _validate_path(self, url):
        match = _contains_disallowed_url_pchar_re.search(url)
        if match:
            raise InvalidURL(
                f"URL can't contain control characters. {url!r} "
                f"(found at least {match.group()!r})"
            )

    def _validate_host(self, host):
        match = _contains_disallowed_url_pchar_re.search(host)
        if match:
            raise InvalidURL(
                f"URL can't contain control characters. {host!r} "
                f"(found at least {match.group()!r})"
            )

    def putheader(self, header, *values):
        if self.__state != _CS_REQ_STARTED:
            raise CannotSendHeader()

        if hasattr(header, 'encode'):
            header = header.encode('ascii')

        if not _is_legal_header_name(header):
            raise ValueError('Invalid header name %r' % (header,))

        values = list(values)
        for i, one_value in enumerate(values):
            if hasattr(one_value, 'encode'):
                values[i] = one_value.encode('latin-1')
            elif isinstance(one_value, int):
                values[i] = str(one_value).encode('ascii')

            if _is_illegal_header_value(values[i]):
                raise ValueError('Invalid header value %r' % (values[i],))

        value = b'\r\n\t'.join(values)
        header = header + b': ' + value
        self._output(header)

    def endheaders(self, message_body=None, *, encode_chunked=False):
        if self.__state == _CS_REQ_STARTED:
            self.__state = _CS_REQ_SENT
        else:
            raise CannotSendHeader()
        self._send_output(message_body, encode_chunked=encode_chunked)

    def request(
        self, method, url, body=None, headers={}, *, encode_chunked=False
    ):
        """Send a complete request to the server."""
        self._send_request(method, url, body, headers, encode_chunked)

    def _send_request(self, method, url, body, headers, encode_chunked):
        header_names = frozenset(k.lower() for k in headers)
        skips = {}
        if 'host' in header_names:
            skips['skip_host'] = 1
        if 'accept-encoding' in header_names:
            skips['skip_accept_encoding'] = 1

        self.putrequest(method, url, **skips)

        if 'content-length' not in header_names:
            if 'transfer-encoding' not in header_names:
                encode_chunked = False
                content_length = self._get_content_length(body, method)
                if content_length is None:
                    if body is not None:
                        if self.debuglevel > 0:
                            print('Unable to determine size of %r' % body)
                        encode_chunked = True
                        self.putheader('Transfer-Encoding', 'chunked')
                else:
                    self.putheader('Content-Length', str(content_length))
        else:
            encode_chunked = False

        for hdr, value in headers.items():
            self.putheader(hdr, value)
        if isinstance(body, str):
            body = _encode(body, 'body')
        self.endheaders(body, encode_chunked=encode_chunked)

    def getresponse(self):
        if self.__response and self.__response.isclosed():
            self.__response = None

        if self.__state != _CS_REQ_SENT or self.__response:
            raise ResponseNotReady(self.__state)

        if self.debuglevel > 0:
            response = self.response_class(
                self.sock, self.debuglevel, method=self._method
            )
        else:
            response = self.response_class(self.sock, method=self._method)

        try:
            try:
                response.begin()
            except ConnectionError:
                self.close()
                raise
            assert response.will_close != _UNKNOWN
            self.__state = _CS_IDLE

            if response.will_close:
                self.close()
            else:
                self.__response = response

            return response
        except BaseException:
            response.close()
            raise


try:
    import ssl
except ImportError:
    pass
else:
    def _create_https_context(http_version):
        context = ssl._create_default_https_context()
        if http_version == 11:
            context.set_alpn_protocols(['http/1.1'])
        return context

    class HTTPSConnection(HTTPConnection):
        """This class allows communication via SSL."""

        default_port = HTTPS_PORT

        def __init__(
            self,
            host,
            port=None,
            *,
            timeout=socket._GLOBAL_DEFAULT_TIMEOUT,
            source_address=None,
            context=None,
            blocksize=8192,
        ):
            super().__init__(
                host,
                port,
                timeout,
                source_address,
                blocksize=blocksize,
            )
            if context is None:
                context = _create_https_context(self._http_vsn)
            self._context = context

        def connect(self):
            """Connect to a host on a given (SSL) port."""
            super().connect()
            if self._tunnel_host:
                server_hostname = self._tunnel_host
            else:
                server_hostname = self.host
            self.sock = self._context.wrap_socket(
                self.sock,
                server_hostname=server_hostname,
            )

    __all__.append("HTTPSConnection")


class HTTPException(Exception):
    pass


class NotConnected(HTTPException):
    pass


class InvalidURL(HTTPException):
    pass


class UnknownProtocol(HTTPException):
    def __init__(self, version):
        self.args = (version,)
        self.version = version


class UnknownTransferEncoding(HTTPException):
    pass


class UnimplementedFileMode(HTTPException):
    pass


class IncompleteRead(HTTPException):
    def __init__(self, partial, expected=None):
        self.args = (partial,)
        self.partial = partial
        self.expected = expected

    def __repr__(self):
        if self.expected is not None:
            e = ', %i more expected' % self.expected
        else:
            e = ''
        return '%s(%i bytes read%s)' % (
            self.__class__.__name__,
            len(self.partial),
            e,
        )

    def __str__(self):
        return self.__repr__()


class ImproperConnectionState(HTTPException):
    pass


class CannotSendRequest(ImproperConnectionState):
    pass


class CannotSendHeader(ImproperConnectionState):
    pass


class ResponseNotReady(ImproperConnectionState):
    pass


class BadStatusLine(HTTPException):
    def __init__(self, line):
        if not line:
            line = repr(line)
        self.args = (line,)
        self.line = line


class LineTooLong(HTTPException):
    def __init__(self, line_type):
        HTTPException.__init__(
            self,
            "got more than %d bytes when reading %s" % (_MAXLINE, line_type),
        )


class RemoteDisconnected(ConnectionResetError, BadStatusLine):
    def __init__(self, *pos, **kw):
        BadStatusLine.__init__(self, "")
        ConnectionResetError.__init__(self, *pos, **kw)


# for backwards compatibility
error = HTTPException
