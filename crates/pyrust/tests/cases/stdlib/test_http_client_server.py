import io
import os
import socket
import tempfile
import http
import http.client as hc
import http.server as hs

# ---------------------------------------------------------------------------
# 1. http.client constants, exceptions, HTTPMessage, parse_headers
# ---------------------------------------------------------------------------
assert hc.HTTP_PORT == 80
assert hc.HTTPS_PORT == 443
assert hc.responses[200] == "OK"
assert hc.responses[404] == "Not Found"
assert hc.responses[418] == "I'm a Teapot"
assert hc.OK == 200 and hc.NOT_FOUND == 404

assert issubclass(hc.HTTPException, Exception)
assert hc.error is hc.HTTPException
assert issubclass(hc.NotConnected, hc.HTTPException)
assert issubclass(hc.InvalidURL, hc.HTTPException)
assert issubclass(hc.UnknownProtocol, hc.HTTPException)
assert issubclass(hc.UnknownTransferEncoding, hc.HTTPException)
assert issubclass(hc.UnimplementedFileMode, hc.HTTPException)
assert issubclass(hc.IncompleteRead, hc.HTTPException)
assert issubclass(hc.ImproperConnectionState, hc.HTTPException)
assert issubclass(hc.CannotSendRequest, hc.ImproperConnectionState)
assert issubclass(hc.CannotSendHeader, hc.ImproperConnectionState)
assert issubclass(hc.ResponseNotReady, hc.ImproperConnectionState)
assert issubclass(hc.BadStatusLine, hc.HTTPException)
assert issubclass(hc.LineTooLong, hc.HTTPException)
assert issubclass(hc.RemoteDisconnected, ConnectionResetError)
assert issubclass(hc.RemoteDisconnected, hc.BadStatusLine)

ir = hc.IncompleteRead(b"abc", 5)
assert ir.partial == b"abc" and ir.expected == 5
assert str(ir) == "IncompleteRead(3 bytes read, 5 more expected)"
assert repr(hc.IncompleteRead(b"ab")) == "IncompleteRead(2 bytes read)"
assert hc.UnknownProtocol("HTTP/9.9").version == "HTTP/9.9"
assert hc.BadStatusLine("GARBAGE").line == "GARBAGE"
assert "header line" in str(hc.LineTooLong("header line"))

raw_hdrs = io.BytesIO(
    b"Content-Type: text/plain; charset=utf-8\r\n"
    b"Set-Cookie: a=1\r\n"
    b"set-cookie: b=2\r\n"
    b"X-Folded: first\r\n"
    b"  second\r\n"
    b"\r\n"
    b"BODY"
)
msg = hc.parse_headers(raw_hdrs)
assert isinstance(msg, hc.HTTPMessage)
assert len(msg) == 4
assert "content-type" in msg
assert "CONTENT-TYPE" in msg
assert "missing" not in msg
assert msg["content-type"] == "text/plain; charset=utf-8"
assert msg["missing"] is None
assert msg.get("Set-Cookie") == "a=1"
assert msg.get("missing", "fallback") == "fallback"
assert msg.get_all("set-cookie") == ["a=1", "b=2"]
assert msg.get_all("missing") is None
assert msg["x-folded"] == "first\r\n  second"
assert msg.keys() == ["Content-Type", "Set-Cookie", "set-cookie", "X-Folded"]
assert msg.items() == [
    ("Content-Type", "text/plain; charset=utf-8"),
    ("Set-Cookie", "a=1"),
    ("set-cookie", "b=2"),
    ("X-Folded", "first\r\n  second"),
]
assert "Content-Type: text/plain; charset=utf-8\n" in msg.as_string()
assert raw_hdrs.read() == b"BODY"

msg["X-Extra"] = "yes"
assert msg["x-extra"] == "yes"
del msg["set-cookie"]
assert "Set-Cookie" not in msg

# Oversized header line -> LineTooLong
try:
    hc.parse_headers(io.BytesIO(b"X-Big: " + b"a" * 70000 + b"\r\n\r\n"))
    assert False
except hc.LineTooLong:
    pass

# Too many headers -> HTTPException
try:
    hc.parse_headers(io.BytesIO(b"X-H: 1\r\n" * 105 + b"\r\n"))
    assert False
except hc.HTTPException:
    pass

print("client_headers_and_exceptions_ok")


# ---------------------------------------------------------------------------
# 2. HTTPResponse framing & error cases via in-memory fake socket
# ---------------------------------------------------------------------------
class _FakeStream(io.BytesIO):
    def peek(self, size=0):
        pos = self.tell()
        data = self.read(size if size > 0 else 8192)
        self.seek(pos)
        return data


class FakeSocket:
    def __init__(self, payload):
        self._payload = payload

    def makefile(self, mode="rb", buffering=None):
        return _FakeStream(self._payload)


# 2a. Content-Length response: partial read, readinto, readline, context manager
resp = hc.HTTPResponse(
    FakeSocket(
        b"HTTP/1.1 200 OK\r\n"
        b"Content-Length: 16\r\n"
        b"X-Multi: one\r\n"
        b"X-Multi: two\r\n"
        b"\r\n"
        b"line1\nline2\n1234EXTRA"
    ),
    method="GET",
)
with resp:
    try:
        resp.getheader("X-Multi")
        assert False
    except hc.ResponseNotReady:
        pass
    resp.begin()
    assert resp.status == 200
    assert resp.reason == "OK"
    assert resp.version == 11
    assert resp.getheader("x-multi") == "one, two"
    assert resp.getheader("absent", "d") == "d"
    assert resp.getheaders() == [
        ("Content-Length", "16"),
        ("X-Multi", "one"),
        ("X-Multi", "two"),
    ]
    assert resp.readline() == b"line1\n"
    buf = bytearray(6)
    n = resp.readinto(buf)
    assert n == 6 and bytes(buf) == b"line2\n"
    assert resp.read() == b"1234"
    assert resp.read() == b""
assert resp.closed

# 2b. Chunked transfer encoding (multiple chunks, extension, trailer)
chunked_wire = (
    b"HTTP/1.1 200 OK\r\n"
    b"Transfer-Encoding: chunked\r\n"
    b"\r\n"
    b"5;ext=1\r\nHello\r\n"
    b"7\r\n, chunk\r\n"
    b"A\r\ned world!\n\r\n"
    b"0\r\n"
    b"X-Trailer: done\r\n"
    b"\r\n"
)
resp_ch = hc.HTTPResponse(FakeSocket(chunked_wire), method="GET")
resp_ch.begin()
assert resp_ch.chunked is True
assert resp_ch.read(7) == b"Hello, "
buf2 = bytearray(8)
assert resp_ch.readinto(buf2) == 8 and bytes(buf2) == b"chunked "
assert resp_ch.readline() == b"world!\n"
assert resp_ch.read() == b""
resp_ch.close()

# 2c. 100 Continue followed by 201 Created
resp_100 = hc.HTTPResponse(
    FakeSocket(
        b"HTTP/1.1 100 Continue\r\n\r\n"
        b"HTTP/1.1 201 Created\r\n"
        b"Content-Length: 2\r\n\r\n"
        b"ok"
    ),
    method="POST",
)
resp_100.begin()
assert resp_100.status == 201 and resp_100.reason == "Created"
assert resp_100.read() == b"ok"
resp_100.close()

# 2d. HEAD, 204 No Content, 304 Not Modified (bodyless)
resp_head = hc.HTTPResponse(
    FakeSocket(b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\nignored"),
    method="HEAD",
)
resp_head.begin()
assert resp_head.status == 200 and resp_head.read() == b""
resp_head.close()

resp_204 = hc.HTTPResponse(
    FakeSocket(b"HTTP/1.1 204 No Content\r\n\r\n"),
    method="DELETE",
)
resp_204.begin()
assert resp_204.status == 204 and resp_204.length == 0 and resp_204.read() == b""
resp_204.close()

resp_304 = hc.HTTPResponse(
    FakeSocket(b"HTTP/1.1 304 Not Modified\r\n\r\n"),
    method="GET",
)
resp_304.begin()
assert resp_304.status == 304 and resp_304.length == 0 and resp_304.read() == b""
resp_304.close()

# 2e. HTTP/1.0 connection-close framing (no Content-Length)
resp_10 = hc.HTTPResponse(
    FakeSocket(b"HTTP/1.0 200 OK\r\n\r\nuntil eof"),
    method="GET",
)
resp_10.begin()
assert resp_10.version == 10 and resp_10.will_close is True
assert resp_10.read() == b"until eof"
resp_10.close()

# 2f. Protocol error cases
for bad_payload, exc_cls in [
    (b"", hc.RemoteDisconnected),
    (b"NOT_HTTP 200 OK\r\n\r\n", hc.BadStatusLine),
    (b"HTTP/1.1 abc OK\r\n\r\n", hc.BadStatusLine),
    (b"HTTP/2.0 200 OK\r\n\r\n", hc.UnknownProtocol),
]:
    r_err = hc.HTTPResponse(FakeSocket(bad_payload))
    try:
        r_err.begin()
        assert False, f"expected {exc_cls.__name__}"
    except exc_cls:
        pass

# Truncated Content-Length -> IncompleteRead
r_inc = hc.HTTPResponse(
    FakeSocket(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort")
)
r_inc.begin()
try:
    r_inc.read()
    assert False
except hc.IncompleteRead as exc:
    assert exc.partial == b"short" and exc.expected == 5

print("client_response_framing_ok")


# ---------------------------------------------------------------------------
# 3. HTTPConnection URL parsing, header validation, and state machine
# ---------------------------------------------------------------------------
c_host = hc.HTTPConnection("example.com")
assert (c_host.host, c_host.port) == ("example.com", 80)
c_port = hc.HTTPConnection("example.com:8080")
assert (c_port.host, c_port.port) == ("example.com", 8080)
c_v6 = hc.HTTPConnection("[::1]:9090")
assert (c_v6.host, c_v6.port) == ("::1", 9090)

try:
    hc.HTTPConnection("example.com:notaport")
    assert False
except hc.InvalidURL:
    pass

try:
    hc.HTTPConnection("bad\r\nhost")
    assert False
except hc.InvalidURL:
    pass

c_state = hc.HTTPConnection("127.0.0.1", 80)
try:
    c_state.putheader("X-A", "1")
    assert False
except hc.CannotSendHeader:
    pass

try:
    c_state.getresponse()
    assert False
except hc.ResponseNotReady:
    pass

c_state.putrequest("GET", "/ok")
try:
    c_state.putrequest("GET", "/again")
    assert False
except hc.CannotSendRequest:
    pass

for bad_name in ("Bad:Header", " LeadingSpace", "Bad\r\nHeader"):
    try:
        c_state.putheader(bad_name, "val")
        assert False
    except ValueError:
        pass

try:
    c_state.putheader("X-Bad", "val\r\nInjected: 1")
    assert False
except ValueError:
    pass

c_state.close()

try:
    c_state.putrequest("GET", "/bad path")
    assert False
except hc.InvalidURL:
    pass
c_state.close()

try:
    c_state.putrequest("G\r\nET", "/ok")
    assert False
except ValueError:
    pass
c_state.close()

print("client_validation_ok")


# ---------------------------------------------------------------------------
# 4. Single-threaded loopback: HTTPConnection <-> HTTPServer + BaseHTTPRequestHandler
# ---------------------------------------------------------------------------
class _StopServe(Exception):
    pass


class _BoundedHTTPServer(hs.HTTPServer):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self._stop_after_next = False

    def service_actions(self):
        if self._stop_after_next:
            raise _StopServe()


class EchoHandler(hs.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):
        pass

    def do_GET(self):
        if self.path == "/empty":
            self.send_response(http.HTTPStatus.NO_CONTENT)
            self.send_header("Connection", "close")
            self.end_headers()
            return
        if self.path == "/stop":
            body = b"bye"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            self.server._stop_after_next = True
            return
        body = f"GET {self.path} hdr={self.headers.get('X-Custom', '')}".encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("X-Echo", "ok")
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def do_HEAD(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", "42")
        self.send_header("Connection", "close")
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        data = self.rfile.read(length) if length > 0 else b""
        resp_body = b"ECHO:" + data
        self.send_response(201)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(resp_body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(resp_body)


# Verify date_time_string formatting
assert hs.BaseHTTPRequestHandler.date_time_string(None, 0) == "Thu, 01 Jan 1970 00:00:00 GMT"

with _BoundedHTTPServer(("127.0.0.1", 0), EchoHandler) as srv:
    host, port = srv.server_address
    assert host == "127.0.0.1" and port > 0
    assert srv.server_port == port
    assert isinstance(srv.server_name, str) and len(srv.server_name) > 0

    # 4a. GET request
    conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        conn.request("GET", "/hello?a=1", headers={"X-Custom": "pyrust"})
        srv.handle_request()
        r = conn.getresponse()
        assert r.status == 200 and r.reason == "OK"
        assert r.getheader("X-Echo") == "ok"
        assert r.getheader("Server").startswith("BaseHTTP/")
        assert r.getheader("Date").endswith("GMT")
        assert r.read() == b"GET /hello?a=1 hdr=pyrust"
    finally:
        conn.close()

    # 4b. POST request with body
    conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        conn.request("POST", "/submit", body=b"payload-123", headers={"Content-Type": "text/plain"})
        srv.handle_request()
        r = conn.getresponse()
        assert r.status == 201 and r.reason == "Created"
        assert r.read() == b"ECHO:payload-123"
    finally:
        conn.close()

    # 4c. HEAD request
    conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        conn.request("HEAD", "/hello")
        srv.handle_request()
        r = conn.getresponse()
        assert r.status == 200
        assert r.getheader("Content-Length") == "42"
        assert r.read() == b""
    finally:
        conn.close()

    # 4d. 204 No Content
    conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        conn.request("GET", "/empty")
        srv.handle_request()
        r = conn.getresponse()
        assert r.status == 204 and r.read() == b""
    finally:
        conn.close()

    # 4e. Unsupported method -> 501 Not Implemented
    conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        conn.request("DELETE", "/nope")
        srv.handle_request()
        r = conn.getresponse()
        assert r.status == 501
        err_body = r.read()
        assert b"501" in err_body
    finally:
        conn.close()

    # 4f. Manual putrequest / putheader / endheaders / send flow
    conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        conn.connect()
        conn.putrequest("POST", "/manual")
        conn.putheader("Content-Length", "5")
        conn.endheaders()
        conn.send(b"hello")
        srv.handle_request()
        r = conn.getresponse()
        assert r.status == 201 and r.read() == b"ECHO:hello"
    finally:
        conn.close()

    # 4g. Bad HTTP request syntax -> 400 Bad Request
    with socket.create_connection(("127.0.0.1", port), timeout=5) as raw_cli:
        raw_cli.sendall(b"GET / extra HTTP/1.1\r\n\r\n")
        srv.handle_request()
        raw_resp = raw_cli.recv(4096)
        assert raw_resp.startswith(b"HTTP/1.1 400 ")

    # 4h. serve_forever() + service_actions() + shutdown()
    conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        conn.request("GET", "/stop")
        try:
            srv.serve_forever(poll_interval=0.02)
        except _StopServe:
            pass
        srv.shutdown()
        r = conn.getresponse()
        assert r.status == 200 and r.read() == b"bye"
    finally:
        conn.close()

# Verify log_message control character escaping
class _LogProbe(hs.BaseHTTPRequestHandler):
    def __init__(self):
        self.client_address = ("127.0.0.1", 12345)

    def log_date_time_string(self):
        return "01/Jan/1970 00:00:00"

import contextlib
_log_buf = io.StringIO()
with contextlib.redirect_stderr(_log_buf):
    _LogProbe().log_message("hello %s", "a\x00b\nc")
assert _log_buf.getvalue() == r"127.0.0.1 - - [01/Jan/1970 00:00:00] hello a\x00b\x0ac" + "\n"

print("loopback_base_server_ok")


# ---------------------------------------------------------------------------
# 5. Single-threaded loopback: SimpleHTTPRequestHandler (static files & dirs)
# ---------------------------------------------------------------------------
with tempfile.TemporaryDirectory() as tmpdir:
    with open(os.path.join(tmpdir, "hello.txt"), "wb") as f:
        f.write(b"Hello static file!\n")
    sub = os.path.join(tmpdir, "sub")
    os.mkdir(sub)
    with open(os.path.join(sub, "index.html"), "wb") as f:
        f.write(b"<h1>Sub Index</h1>")
    nodir = os.path.join(tmpdir, "listing")
    os.mkdir(nodir)
    with open(os.path.join(nodir, "a.json"), "wb") as f:
        f.write(b'{"k": 1}')

    class QuietStaticHandler(hs.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=tmpdir, **kwargs)

        def log_message(self, format, *args):
            pass

    with hs.HTTPServer(("127.0.0.1", 0), QuietStaticHandler) as srv:
        _, port = srv.server_address

        # 5a. GET static file
        conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            conn.request("GET", "/hello.txt")
            srv.handle_request()
            r = conn.getresponse()
            assert r.status == 200
            assert r.getheader("Content-type").startswith("text/plain")
            assert r.getheader("Content-Length") == "19"
            last_mod = r.getheader("Last-Modified")
            assert last_mod and last_mod.endswith("GMT")
            assert r.read() == b"Hello static file!\n"
        finally:
            conn.close()

        # 5b. HEAD static file
        conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            conn.request("HEAD", "/hello.txt")
            srv.handle_request()
            r = conn.getresponse()
            assert r.status == 200
            assert r.getheader("Content-Length") == "19"
            assert r.read() == b""
        finally:
            conn.close()

        # 5c. Conditional GET (If-Modified-Since -> 304)
        conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            conn.request("GET", "/hello.txt", headers={"If-Modified-Since": last_mod})
            srv.handle_request()
            r = conn.getresponse()
            assert r.status == 304 and r.read() == b""
        finally:
            conn.close()

        # 5d. Directory without trailing slash -> 301 redirect
        conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            conn.request("GET", "/sub")
            srv.handle_request()
            r = conn.getresponse()
            assert r.status == 301
            assert r.getheader("Location") == "/sub/"
            assert r.read() == b""
        finally:
            conn.close()

        # 5e. Directory with index.html -> serves index.html
        conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            conn.request("GET", "/sub/")
            srv.handle_request()
            r = conn.getresponse()
            assert r.status == 200
            assert r.getheader("Content-type").startswith("text/html")
            assert r.read() == b"<h1>Sub Index</h1>"
        finally:
            conn.close()

        # 5f. Directory listing (no index.html)
        conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            conn.request("GET", "/listing/")
            srv.handle_request()
            r = conn.getresponse()
            assert r.status == 200
            listing_html = r.read()
            assert b"Directory listing for /listing/" in listing_html
            assert b'href="a.json"' in listing_html
        finally:
            conn.close()

        # 5g. Missing file -> 404
        conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            conn.request("GET", "/does-not-exist.txt")
            srv.handle_request()
            r = conn.getresponse()
            assert r.status == 404
            assert b"404" in r.read()
        finally:
            conn.close()

        # 5h. Path traversal attempt stays inside directory -> resolves to /hello.txt safely
        conn = hc.HTTPConnection("127.0.0.1", port, timeout=5)
        try:
            conn.request("GET", "/../../../hello.txt")
            srv.handle_request()
            r = conn.getresponse()
            assert r.status == 200
            assert r.read() == b"Hello static file!\n"
        finally:
            conn.close()

print("loopback_simple_server_ok")


# ---------------------------------------------------------------------------
# 6. Chunked request upload + chunked response over raw TCP loopback
# ---------------------------------------------------------------------------
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as raw_srv:
    raw_srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    raw_srv.bind(("127.0.0.1", 0))
    raw_srv.listen(1)
    _, raw_port = raw_srv.getsockname()

    conn = hc.HTTPConnection("127.0.0.1", raw_port, timeout=5)
    try:
        conn.request("POST", "/upload", body=iter([b"part1-", b"part2"]))
        peer, _ = raw_srv.accept()
        with peer:
            peer.settimeout(5)
            received = b""
            while b"0\r\n\r\n" not in received:
                chunk = peer.recv(4096)
                if not chunk:
                    break
                received += chunk
            assert b"Transfer-Encoding: chunked\r\n" in received
            assert b"6\r\npart1-\r\n5\r\npart2\r\n0\r\n\r\n" in received
            peer.sendall(
                b"HTTP/1.1 200 OK\r\n"
                b"Transfer-Encoding: chunked\r\n"
                b"Connection: close\r\n"
                b"\r\n"
                b"4\r\ndone\r\n"
                b"0\r\n\r\n"
            )
        r = conn.getresponse()
        assert r.status == 200 and r.chunked is True
        assert r.read() == b"done"
    finally:
        conn.close()

print("chunked_loopback_ok")
print("http_client_server_ok")
