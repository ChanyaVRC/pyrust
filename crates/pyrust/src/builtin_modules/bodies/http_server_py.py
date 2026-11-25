"""HTTP server classes."""

__version__ = "0.6"

__all__ = [
    "HTTPServer",
    "ThreadingHTTPServer",
    "BaseHTTPRequestHandler",
    "SimpleHTTPRequestHandler",
]

import datetime
import html
import http.client
import io
import itertools
import os
import shutil
import socket
import socketserver
import sys
import time
import urllib.parse

from http import HTTPStatus

DEFAULT_ERROR_MESSAGE = """\
<!DOCTYPE HTML>
<html lang="en">
    <head>
        <meta charset="utf-8">
        <title>Error response</title>
    </head>
    <body>
        <h1>Error response</h1>
        <p>Error code: %(code)d</p>
        <p>Message: %(message)s.</p>
        <p>Error code explanation: %(code)s - %(explain)s.</p>
    </body>
</html>
"""

DEFAULT_ERROR_CONTENT_TYPE = "text/html;charset=utf-8"

_WEEKDAYNAME = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']
_MONTHNAME = [
    None,
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
]
_MONTH_TO_NUM = {
    'jan': 1, 'feb': 2, 'mar': 3, 'apr': 4, 'may': 5, 'jun': 6,
    'jul': 7, 'aug': 8, 'sep': 9, 'oct': 10, 'nov': 11, 'dec': 12,
}

_COMMON_MIME_TYPES = {
    '.html': 'text/html',
    '.htm': 'text/html',
    '.txt': 'text/plain',
    '.text': 'text/plain',
    '.css': 'text/css',
    '.csv': 'text/csv',
    '.js': 'text/javascript',
    '.mjs': 'text/javascript',
    '.json': 'application/json',
    '.xml': 'application/xml',
    '.pdf': 'application/pdf',
    '.zip': 'application/zip',
    '.tar': 'application/x-tar',
    '.wasm': 'application/wasm',
    '.png': 'image/png',
    '.jpg': 'image/jpeg',
    '.jpeg': 'image/jpeg',
    '.gif': 'image/gif',
    '.svg': 'image/svg+xml',
    '.ico': 'image/vnd.microsoft.icon',
    '.webp': 'image/webp',
    '.py': 'text/x-python',
    '.c': 'text/plain',
    '.h': 'text/plain',
    '.md': 'text/markdown',
    '.mp3': 'audio/mpeg',
    '.wav': 'audio/x-wav',
    '.mp4': 'video/mp4',
}


def _format_http_date(timestamp):
    year, month, day, hh, mm, ss, wd, _y, _z = time.gmtime(timestamp)
    return "%s, %02d %s %04d %02d:%02d:%02d GMT" % (
        _WEEKDAYNAME[wd], day, _MONTHNAME[month], year, hh, mm, ss
    )


def _parse_http_date(value):
    """Parse RFC 2822 / IMF-fixdate HTTP date string to UTC datetime."""
    if not isinstance(value, str):
        raise TypeError("expected str")
    s = value.strip()
    if ',' in s:
        s = s.split(',', 1)[1].strip()
    parts = s.split()
    if len(parts) < 5:
        raise ValueError("invalid date")
    day = int(parts[0])
    month = _MONTH_TO_NUM.get(parts[1].lower()[:3])
    if month is None:
        raise ValueError("invalid month")
    year = int(parts[2])
    if year < 100:
        year += 2000 if year < 50 else 1900
    hms = parts[3].split(':')
    if len(hms) != 3:
        raise ValueError("invalid time")
    hh, mm, ss = int(hms[0]), int(hms[1]), int(hms[2])
    tz = parts[4].upper()
    if tz in ('GMT', 'UTC', 'UT', 'Z', '+0000', '-0000'):
        tzinfo = datetime.timezone.utc
    else:
        tzinfo = None
    return datetime.datetime(year, month, day, hh, mm, ss, tzinfo=tzinfo)


def _posix_normpath(path):
    """Normalize a POSIX-style URL path (matching posixpath.normpath)."""
    if not path:
        return '.'
    initial_slashes = path.startswith('/')
    if (
        initial_slashes
        and path.startswith('//')
        and not path.startswith('///')
    ):
        initial_slashes = 2
    else:
        initial_slashes = 1 if initial_slashes else 0
    comps = path.split('/')
    new_comps = []
    for comp in comps:
        if comp in ('', '.'):
            continue
        if (
            comp != '..'
            or (not initial_slashes and not new_comps)
            or (new_comps and new_comps[-1] == '..')
        ):
            new_comps.append(comp)
        elif new_comps:
            new_comps.pop()
    path = ('/' * initial_slashes) + '/'.join(new_comps)
    return path or '.'


def _posix_splitext(path):
    """Split extension from a POSIX-style path."""
    sep_index = path.rfind('/')
    dot_index = path.rfind('.')
    if dot_index > sep_index:
        filename_index = sep_index + 1
        while filename_index < dot_index:
            if path[filename_index] != '.':
                return path[:dot_index], path[dot_index:]
            filename_index += 1
    return path, ''


class HTTPServer(socketserver.TCPServer):
    """TCP-backed HTTP server compatible with CPython's http.server.HTTPServer."""

    allow_reuse_address = 1

    def server_bind(self):
        """Override server_bind to store the server name."""
        socketserver.TCPServer.server_bind(self)
        host, port = self.server_address[:2]
        self.server_name = socket.getfqdn(host)
        self.server_port = port


class ThreadingHTTPServer(socketserver.ThreadingMixIn, HTTPServer):
    """Threading HTTP server subclass."""

    daemon_threads = True


class BaseHTTPRequestHandler(socketserver.StreamRequestHandler):
    """HTTP request handler base class."""

    sys_version = "Python/" + sys.version.split()[0]
    server_version = "BaseHTTP/" + __version__
    error_message_format = DEFAULT_ERROR_MESSAGE
    error_content_type = DEFAULT_ERROR_CONTENT_TYPE
    default_request_version = "HTTP/0.9"
    protocol_version = "HTTP/1.0"
    MessageClass = http.client.HTTPMessage

    weekdayname = list(_WEEKDAYNAME)
    monthname = list(_MONTHNAME)

    responses = {
        v: (v.phrase, v.description)
        for v in HTTPStatus
    }

    _control_char_table = str.maketrans(
        {c: fr'\x{c:02x}' for c in itertools.chain(range(0x20), range(0x7f, 0xa0))}
    )
    _control_char_table[ord('\\')] = r'\\'

    def parse_request(self):
        self.command = None
        self.request_version = version = self.default_request_version
        self.close_connection = True
        requestline = str(self.raw_requestline, 'iso-8859-1')
        requestline = requestline.rstrip('\r\n')
        self.requestline = requestline
        words = requestline.split()
        if len(words) == 0:
            return False

        if len(words) >= 3:
            version = words[-1]
            try:
                if not version.startswith('HTTP/'):
                    raise ValueError
                base_version_number = version.split('/', 1)[1]
                version_number = base_version_number.split(".")
                if len(version_number) != 2:
                    raise ValueError
                if any(not component.isdigit() for component in version_number):
                    raise ValueError("non digit in http version")
                if any(len(component) > 10 for component in version_number):
                    raise ValueError("unreasonable length http version")
                version_number = int(version_number[0]), int(version_number[1])
            except (ValueError, IndexError):
                self.send_error(
                    HTTPStatus.BAD_REQUEST,
                    "Bad request version (%r)" % version,
                )
                return False
            if version_number >= (1, 1) and self.protocol_version >= "HTTP/1.1":
                self.close_connection = False
            if version_number >= (2, 0):
                self.send_error(
                    HTTPStatus.HTTP_VERSION_NOT_SUPPORTED,
                    "Invalid HTTP version (%s)" % base_version_number,
                )
                return False
            self.request_version = version

        if not 2 <= len(words) <= 3:
            self.send_error(
                HTTPStatus.BAD_REQUEST,
                "Bad request syntax (%r)" % requestline,
            )
            return False
        command, path = words[:2]
        if len(words) == 2:
            self.close_connection = True
            if command != 'GET':
                self.send_error(
                    HTTPStatus.BAD_REQUEST,
                    "Bad HTTP/0.9 request type (%r)" % command,
                )
                return False
        self.command, self.path = command, path

        if self.path.startswith('//'):
            self.path = '/' + self.path.lstrip('/')

        try:
            self.headers = http.client.parse_headers(
                self.rfile, _class=self.MessageClass
            )
        except http.client.LineTooLong as err:
            self.send_error(
                HTTPStatus.REQUEST_HEADER_FIELDS_TOO_LARGE,
                "Line too long",
                str(err),
            )
            return False
        except http.client.HTTPException as err:
            self.send_error(
                HTTPStatus.REQUEST_HEADER_FIELDS_TOO_LARGE,
                "Too many headers",
                str(err),
            )
            return False

        conntype = self.headers.get('Connection', "")
        if conntype.lower() == 'close':
            self.close_connection = True
        elif (
            conntype.lower() == 'keep-alive'
            and self.protocol_version >= "HTTP/1.1"
        ):
            self.close_connection = False

        expect = self.headers.get('Expect', "")
        if (
            expect.lower() == "100-continue"
            and self.protocol_version >= "HTTP/1.1"
            and self.request_version >= "HTTP/1.1"
        ):
            if not self.handle_expect_100():
                return False
        return True

    def handle_expect_100(self):
        self.send_response_only(HTTPStatus.CONTINUE)
        self.end_headers()
        return True

    def handle_one_request(self):
        try:
            self.raw_requestline = self.rfile.readline(65537)
            if len(self.raw_requestline) > 65536:
                self.requestline = ''
                self.request_version = ''
                self.command = ''
                self.send_error(HTTPStatus.REQUEST_URI_TOO_LONG)
                return
            if not self.raw_requestline:
                self.close_connection = True
                return
            if not self.parse_request():
                return
            mname = 'do_' + self.command
            if not hasattr(self, mname):
                self.send_error(
                    HTTPStatus.NOT_IMPLEMENTED,
                    "Unsupported method (%r)" % self.command,
                )
                return
            method = getattr(self, mname)
            method()
            self.wfile.flush()
        except TimeoutError as e:
            self.log_error("Request timed out: %r", e)
            self.close_connection = True
            return

    def handle(self):
        self.close_connection = True
        self.handle_one_request()
        while not self.close_connection:
            self.handle_one_request()

    def send_error(self, code, message=None, explain=None):
        try:
            shortmsg, longmsg = self.responses[code]
        except KeyError:
            shortmsg, longmsg = '???', '???'
        if message is None:
            message = shortmsg
        if explain is None:
            explain = longmsg
        self.log_error("code %d, message %s", code, message)
        self.send_response(code, message)
        self.send_header('Connection', 'close')

        body = None
        if (
            code >= 200
            and code not in (
                HTTPStatus.NO_CONTENT,
                HTTPStatus.RESET_CONTENT,
                HTTPStatus.NOT_MODIFIED,
            )
        ):
            content = self.error_message_format % {
                'code': code,
                'message': html.escape(message, quote=False),
                'explain': html.escape(explain, quote=False),
            }
            body = content.encode('UTF-8', 'replace')
            self.send_header("Content-Type", self.error_content_type)
            self.send_header('Content-Length', str(len(body)))
        self.end_headers()

        if self.command != 'HEAD' and body:
            self.wfile.write(body)

    def send_response(self, code, message=None):
        self.log_request(code)
        self.send_response_only(code, message)
        self.send_header('Server', self.version_string())
        self.send_header('Date', self.date_time_string())

    def send_response_only(self, code, message=None):
        if self.request_version != 'HTTP/0.9':
            if message is None:
                if code in self.responses:
                    message = self.responses[code][0]
                else:
                    message = ''
            if not hasattr(self, '_headers_buffer'):
                self._headers_buffer = []
            self._headers_buffer.append(
                ("%s %d %s\r\n" % (self.protocol_version, code, message)).encode(
                    'latin-1', 'strict'
                )
            )

    def send_header(self, keyword, value):
        if self.request_version != 'HTTP/0.9':
            if not hasattr(self, '_headers_buffer'):
                self._headers_buffer = []
            self._headers_buffer.append(
                ("%s: %s\r\n" % (keyword, value)).encode('latin-1', 'strict')
            )

        if keyword.lower() == 'connection':
            if value.lower() == 'close':
                self.close_connection = True
            elif value.lower() == 'keep-alive':
                self.close_connection = False

    def end_headers(self):
        if self.request_version != 'HTTP/0.9':
            self._headers_buffer.append(b"\r\n")
            self.flush_headers()

    def flush_headers(self):
        if hasattr(self, '_headers_buffer'):
            self.wfile.write(b"".join(self._headers_buffer))
            self._headers_buffer = []

    def log_request(self, code='-', size='-'):
        if isinstance(code, HTTPStatus):
            code = code.value
        self.log_message(
            '"%s" %s %s', self.requestline, str(code), str(size)
        )

    def log_error(self, format, *args):
        self.log_message(format, *args)

    def log_message(self, format, *args):
        message = format % args
        sys.stderr.write(
            "%s - - [%s] %s\n" % (
                self.address_string(),
                self.log_date_time_string(),
                message.translate(self._control_char_table),
            )
        )

    def version_string(self):
        return self.server_version + ' ' + self.sys_version

    def date_time_string(self, timestamp=None):
        if timestamp is None:
            timestamp = time.time()
        return _format_http_date(timestamp)

    def log_date_time_string(self):
        now = time.time()
        year, month, day, hh, mm, ss, _x, _y, _z = time.localtime(now)
        return "%02d/%3s/%04d %02d:%02d:%02d" % (
            day, self.monthname[month], year, hh, mm, ss
        )

    def address_string(self):
        return self.client_address[0]


class SimpleHTTPRequestHandler(BaseHTTPRequestHandler):
    """Simple HTTP request handler with GET and HEAD commands."""

    server_version = "SimpleHTTP/" + __version__
    index_pages = ("index.html", "index.htm")
    extensions_map = {
        '.gz': 'application/gzip',
        '.Z': 'application/octet-stream',
        '.bz2': 'application/x-bzip2',
        '.xz': 'application/x-xz',
    }

    def __init__(self, *args, directory=None, **kwargs):
        if directory is None:
            directory = os.getcwd()
        self.directory = os.fspath(directory)
        super().__init__(*args, **kwargs)

    def do_GET(self):
        f = self.send_head()
        if f:
            try:
                self.copyfile(f, self.wfile)
            finally:
                f.close()

    def do_HEAD(self):
        f = self.send_head()
        if f:
            f.close()

    def send_head(self):
        path = self.translate_path(self.path)
        f = None
        if os.path.isdir(path):
            parts = urllib.parse.urlsplit(self.path)
            if not parts.path.endswith('/'):
                self.send_response(HTTPStatus.MOVED_PERMANENTLY)
                new_parts = (
                    parts[0], parts[1], parts[2] + '/', parts[3], parts[4]
                )
                new_url = urllib.parse.urlunsplit(new_parts)
                self.send_header("Location", new_url)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return None
            for index in self.index_pages:
                index = os.path.join(path, index)
                if os.path.isfile(index):
                    path = index
                    break
            else:
                return self.list_directory(path)
        ctype = self.guess_type(path)
        if path.endswith("/"):
            self.send_error(HTTPStatus.NOT_FOUND, "File not found")
            return None
        try:
            f = open(path, 'rb')
        except OSError:
            self.send_error(HTTPStatus.NOT_FOUND, "File not found")
            return None

        try:
            fs = os.stat(path)
            if (
                "If-Modified-Since" in self.headers
                and "If-None-Match" not in self.headers
            ):
                try:
                    ims = _parse_http_date(self.headers["If-Modified-Since"])
                except (TypeError, IndexError, OverflowError, ValueError):
                    pass
                else:
                    if ims.tzinfo is None:
                        ims = ims.replace(tzinfo=datetime.timezone.utc)
                    if ims.tzinfo is datetime.timezone.utc:
                        last_modif = datetime.datetime.fromtimestamp(
                            fs.st_mtime, datetime.timezone.utc
                        )
                        last_modif = last_modif.replace(microsecond=0)
                        if last_modif <= ims:
                            self.send_response(HTTPStatus.NOT_MODIFIED)
                            self.end_headers()
                            f.close()
                            return None

            self.send_response(HTTPStatus.OK)
            self.send_header("Content-type", ctype)
            self.send_header("Content-Length", str(fs.st_size))
            self.send_header("Last-Modified", self.date_time_string(fs.st_mtime))
            self.end_headers()
            return f
        except BaseException:
            f.close()
            raise

    def list_directory(self, path):
        try:
            entries = os.listdir(path)
        except OSError:
            self.send_error(
                HTTPStatus.NOT_FOUND,
                "No permission to list directory",
            )
            return None
        entries.sort(key=lambda a: a.lower())
        r = []
        try:
            displaypath = urllib.parse.unquote(
                self.path, errors='surrogatepass'
            )
        except UnicodeDecodeError:
            displaypath = urllib.parse.unquote(self.path)
        displaypath = html.escape(displaypath, quote=False)
        enc = sys.getfilesystemencoding()
        title = f'Directory listing for {displaypath}'
        r.append('<!DOCTYPE HTML>')
        r.append('<html lang="en">')
        r.append('<head>')
        r.append(f'<meta charset="{enc}">')
        r.append(f'<title>{title}</title>\n</head>')
        r.append(f'<body>\n<h1>{title}</h1>')
        r.append('<hr>\n<ul>')
        for name in entries:
            fullname = os.path.join(path, name)
            displayname = linkname = name
            if os.path.isdir(fullname):
                displayname = name + "/"
                linkname = name + "/"
            if os.path.islink(fullname):
                displayname = name + "@"
            r.append(
                '<li><a href="%s">%s</a></li>' % (
                    urllib.parse.quote(linkname, errors='surrogatepass'),
                    html.escape(displayname, quote=False),
                )
            )
        r.append('</ul>\n<hr>\n</body>\n</html>\n')
        encoded = '\n'.join(r).encode(enc, 'surrogateescape')
        f = io.BytesIO()
        f.write(encoded)
        f.seek(0)
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-type", "text/html; charset=%s" % enc)
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        return f

    def translate_path(self, path):
        path = path.split('?', 1)[0]
        path = path.split('#', 1)[0]
        trailing_slash = path.rstrip().endswith('/')
        try:
            path = urllib.parse.unquote(path, errors='surrogatepass')
        except UnicodeDecodeError:
            path = urllib.parse.unquote(path)
        path = _posix_normpath(path)
        words = path.split('/')
        words = filter(None, words)
        path = self.directory
        for word in words:
            if os.path.dirname(word) or word in (os.curdir, os.pardir):
                continue
            path = os.path.join(path, word)
        if trailing_slash:
            path += '/'
        return path

    def copyfile(self, source, outputfile):
        shutil.copyfileobj(source, outputfile)

    def guess_type(self, path):
        _base, ext = _posix_splitext(path)
        if ext in self.extensions_map:
            return self.extensions_map[ext]
        ext = ext.lower()
        if ext in self.extensions_map:
            return self.extensions_map[ext]
        if ext in _COMMON_MIME_TYPES:
            return _COMMON_MIME_TYPES[ext]
        return 'application/octet-stream'
