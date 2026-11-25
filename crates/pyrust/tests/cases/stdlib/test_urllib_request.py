"""Comprehensive offline CPython 3.12 parity tests for urllib.response,
urllib.error, and urllib.request (issue #3223).
"""

import io
import os
import subprocess
import sys
import tempfile
import urllib
import urllib.error
import urllib.parse
import urllib.request
import urllib.response

# ---------------------------------------------------------------------------
# 1. Package & submodule identity
# ---------------------------------------------------------------------------
assert urllib.error is sys.modules["urllib.error"]
assert urllib.response is sys.modules["urllib.response"]
assert urllib.request is sys.modules["urllib.request"]
assert urllib.parse is sys.modules["urllib.parse"]
assert urllib is sys.modules["urllib"]

for name in ("addbase", "addclosehook", "addinfo", "addinfourl"):
    assert name in urllib.response.__all__
    assert hasattr(urllib.response, name)

for name in ("URLError", "HTTPError", "ContentTooShortError"):
    assert name in urllib.error.__all__
    assert hasattr(urllib.error, name)

for name in (
    "Request",
    "OpenerDirector",
    "BaseHandler",
    "HTTPHandler",
    "HTTPRedirectHandler",
    "HTTPErrorProcessor",
    "HTTPDefaultErrorHandler",
    "UnknownHandler",
    "FileHandler",
    "DataHandler",
    "ProxyHandler",
    "build_opener",
    "install_opener",
    "urlopen",
    "urlretrieve",
    "urlcleanup",
    "pathname2url",
    "url2pathname",
    "getproxies",
):
    assert name in urllib.request.__all__, name
    assert hasattr(urllib.request, name), name

assert hasattr(urllib.request, "parse_http_list")
assert hasattr(urllib.request, "parse_keqv_list")

print("module_identity_ok")

# ---------------------------------------------------------------------------
# 2. urllib.response: addbase, addclosehook, addinfo, addinfourl
# ---------------------------------------------------------------------------
raw = io.BytesIO(b"line1\nline2\nline3\n")
base = urllib.response.addbase(raw)
assert base.fp is raw
assert base.readline() == b"line1\n"
assert list(base) == [b"line2\n", b"line3\n"]
assert "addbase" in repr(base)
with base as ctx:
    assert ctx is base
assert raw.closed
try:
    with base:
        assert False, "expected ValueError on closed addbase"
except ValueError:
    pass

hook_calls = []


def _on_close(a, b):
    hook_calls.append((a, b))


raw2 = io.BytesIO(b"hook-data")
hooked = urllib.response.addclosehook(raw2, _on_close, "x", 42)
assert hooked.read() == b"hook-data"
hooked.close()
hooked.close()  # idempotent: hook runs only once
assert hook_calls == [("x", 42)]
assert raw2.closed

hdrs = {"Content-Type": "text/plain"}
info_obj = urllib.response.addinfo(io.BytesIO(b"abc"), hdrs)
assert info_obj.info() is hdrs
assert info_obj.headers is hdrs
assert info_obj.readlines() == [b"abc"]
info_obj.close()

url_obj = urllib.response.addinfourl(
    io.BytesIO(b"body"), hdrs, "http://example.test/x", code=200
)
with url_obj as r:
    assert r.info() is hdrs
    assert r.geturl() == "http://example.test/x"
    assert r.url == "http://example.test/x"
    assert r.code == 200
    assert r.status == 200
    assert r.getcode() == 200
    assert r.read() == b"body"

print("urllib_response_ok")

# ---------------------------------------------------------------------------
# 3. urllib.error: URLError, HTTPError, ContentTooShortError
# ---------------------------------------------------------------------------
assert issubclass(urllib.error.URLError, OSError)
assert issubclass(urllib.error.HTTPError, urllib.error.URLError)
assert issubclass(urllib.error.HTTPError, urllib.response.addinfourl)
assert issubclass(urllib.error.ContentTooShortError, urllib.error.URLError)

uerr = urllib.error.URLError("connection refused", filename="/tmp/x")
assert uerr.reason == "connection refused"
assert uerr.filename == "/tmp/x"
assert str(uerr) == "<urlopen error connection refused>"

herr_fp = io.BytesIO(b"not found body")
herr_hdrs = {"Content-Type": "text/plain", "X-Code": "404"}
herr = urllib.error.HTTPError(
    "http://example.test/missing", 404, "Not Found", herr_hdrs, herr_fp
)
assert herr.code == 404
assert herr.status == 404
assert herr.getcode() == 404
assert herr.msg == "Not Found"
assert herr.reason == "Not Found"
assert herr.headers is herr_hdrs
assert herr.hdrs is herr_hdrs
assert herr.info() is herr_hdrs
assert herr.url == "http://example.test/missing"
assert herr.geturl() == "http://example.test/missing"
assert herr.filename == "http://example.test/missing"
assert str(herr) == "HTTP Error 404: Not Found"
assert repr(herr) == "<HTTPError 404: 'Not Found'>"
with herr as e:
    assert e.read() == b"not found body"
assert herr_fp.closed

herr_none = urllib.error.HTTPError("http://example.test/e", 500, "Err", {}, None)
assert herr_none.read() == b""
herr_none.close()

cts = urllib.error.ContentTooShortError("short", ("/tmp/f", {}))
assert cts.reason == "short"
assert cts.content == ("/tmp/f", {})
assert str(cts) == "<urlopen error short>"

print("urllib_error_ok")

# ---------------------------------------------------------------------------
# 4. Request object
# ---------------------------------------------------------------------------
req = urllib.request.Request(
    "<URL:http://Example.COM:8080/a/b?k=1#frag>",
    headers={"x-custom": "123", "Accept": "text/plain"},
)
assert req.full_url == "http://Example.COM:8080/a/b?k=1#frag"
assert req.get_full_url() == "http://Example.COM:8080/a/b?k=1#frag"
assert req.type == "http"
assert req.host == "Example.COM:8080"
assert req.selector == "/a/b?k=1"
assert req.fragment == "frag"
assert req.origin_req_host == "example.com"
assert req.unverifiable is False
assert req.get_method() == "GET"

assert req.has_header("X-custom")
assert req.get_header("X-custom") == "123"
assert req.get_header("Missing", "fallback") == "fallback"
req.add_unredirected_header("authorization", "Bearer tok")
assert req.has_header("Authorization")
assert req.get_header("Authorization") == "Bearer tok"
items = dict(req.header_items())
assert items["X-custom"] == "123"
assert items["Authorization"] == "Bearer tok"
assert items["Accept"] == "text/plain"
req.remove_header("X-custom")
assert not req.has_header("X-custom")

# data property & Content-length invalidation + method selection
req.add_header("Content-length", "99")
req.data = b"hello=world"
assert not req.has_header("Content-length")
assert req.get_method() == "POST"
del req.data
assert req.data is None
assert req.get_method() == "GET"

req_put = urllib.request.Request("http://example.test/item", data=b"x", method="PUT")
assert req_put.get_method() == "PUT"

# Proxy support on Request
req_p = urllib.request.Request("http://target.test/path")
assert not req_p.has_proxy()
req_p.set_proxy("proxy.local:3128", "http")
assert req_p.has_proxy()
assert req_p.host == "proxy.local:3128"
assert req_p.selector == "http://target.test/path"

# Invalid URL without scheme
try:
    urllib.request.Request("no-scheme-here")
    assert False, "expected ValueError"
except ValueError:
    pass

print("request_ok")

# ---------------------------------------------------------------------------
# 5. Helper functions: pathname2url, url2pathname, parse_http_list,
#    parse_keqv_list, getproxies_environment, proxy_bypass_environment
# ---------------------------------------------------------------------------
assert urllib.request.parse_http_list('a="1,2", b=3, c="x\\"y"') == [
    'a="1,2"',
    'b=3',
    'c="x"y"',
]
assert urllib.request.parse_http_list('realm="my realm", qop="auth,auth-int", nonce=abc') == [
    'realm="my realm"',
    'qop="auth,auth-int"',
    'nonce=abc',
]
assert urllib.request.parse_keqv_list(['realm="my realm"', 'nonce=abc', 'qop="auth"']) == {
    'realm': 'my realm',
    'nonce': 'abc',
    'qop': 'auth',
}

# OS-appropriate pathname2url / url2pathname roundtrip
with tempfile.TemporaryDirectory() as tmpdir:
    sample_path = os.path.join(tmpdir, "hello world.txt")
    url_part = urllib.request.pathname2url(sample_path)
    assert "%20" in url_part
    back_path = urllib.request.url2pathname(url_part)
    assert os.path.normcase(back_path) == os.path.normcase(sample_path)

# Environment proxy parsing & bypass (save/restore env deterministically)
_saved_env = {}
_proxy_keys = ("http_proxy", "HTTP_PROXY", "https_proxy", "no_proxy", "REQUEST_METHOD")
for _k in _proxy_keys:
    _saved_env[_k] = os.environ.get(_k)
try:
    for _k in _proxy_keys:
        if _k in os.environ:
            del os.environ[_k]
    assert urllib.request.getproxies_environment() == {}
    os.environ["HTTP_PROXY"] = "http://upper:8080"
    os.environ["https_proxy"] = "http://secure:8443"
    os.environ["no_proxy"] = "localhost, .example.com, 127.0.0.1"
    px = urllib.request.getproxies_environment()
    assert px["http"] == "http://upper:8080"
    assert px["https"] == "http://secure:8443"
    assert px["no"] == "localhost, .example.com, 127.0.0.1"
    assert urllib.request.proxy_bypass_environment("localhost", px) is True
    assert urllib.request.proxy_bypass_environment("sub.example.com:80", px) is True
    assert urllib.request.proxy_bypass_environment("other.org", px) is False
    assert urllib.request.proxy_bypass_environment("anything.org", {"no": "*"}) is True
    # Lowercase overrides uppercase
    os.environ["http_proxy"] = "http://lower:3128"
    assert urllib.request.getproxies_environment()["http"] == "http://lower:3128"
    # CGI REQUEST_METHOD strips uppercase HTTP_PROXY when lowercase is absent
    if "http_proxy" in os.environ:
        del os.environ["http_proxy"]
    os.environ["REQUEST_METHOD"] = "GET"
    assert "http" not in urllib.request.getproxies_environment()
finally:
    for _k, _v in _saved_env.items():
        if _v is None:
            if _k in os.environ:
                del os.environ[_k]
        else:
            os.environ[_k] = _v

print("helpers_ok")

# ---------------------------------------------------------------------------
# 6. data: URLs via urlopen and urlretrieve
# ---------------------------------------------------------------------------
with urllib.request.urlopen("data:,Hello%2C%20World%21") as resp:
    assert resp.read() == b"Hello, World!"
    assert resp.headers["Content-type"] == "text/plain;charset=US-ASCII"
    assert resp.headers["Content-length"] == "13"
    assert resp.geturl() == "data:,Hello%2C%20World%21"

with urllib.request.urlopen("data:text/html;charset=utf-8,%3Ch1%3EHi%3C/h1%3E") as resp:
    assert resp.read() == b"<h1>Hi</h1>"
    assert resp.headers["Content-type"] == "text/html;charset=utf-8"

with urllib.request.urlopen("data:application/octet-stream;base64,SGVsbG8gV29ybGQ=") as resp:
    assert resp.read() == b"Hello World"
    assert resp.headers["Content-type"] == "application/octet-stream"
    assert resp.headers["Content-length"] == "11"

# urlretrieve into temp file + urlcleanup
tmp_dl, dl_hdrs = urllib.request.urlretrieve("data:text/plain;base64,YWJjZGVm")
assert os.path.isfile(tmp_dl)
with open(tmp_dl, "rb") as f:
    assert f.read() == b"abcdef"
assert dl_hdrs["Content-length"] == "6"
urllib.request.urlcleanup()
assert not os.path.exists(tmp_dl)

print("data_urls_ok")

# ---------------------------------------------------------------------------
# 7. file:// URLs via urlopen and urlretrieve (Linux & Windows)
# ---------------------------------------------------------------------------
def _make_file_url(path):
    p = urllib.request.pathname2url(path)
    if p.startswith("///"):
        return "file:" + p
    return "file://" + p


with tempfile.TemporaryDirectory() as tmpdir:
    fpath = os.path.join(tmpdir, "sample data.json")
    payload = b'{"hello": "file"}'
    with open(fpath, "wb") as f:
        f.write(payload)

    furl = _make_file_url(fpath)
    with urllib.request.urlopen(furl) as resp:
        assert resp.read() == payload
        assert resp.headers["Content-type"] == "application/json"
        assert resp.headers["Content-length"] == str(len(payload))
        assert resp.headers["Last-modified"].endswith("GMT")
        assert resp.geturl().startswith("file://")

    # urlretrieve with explicit destination & reporthook
    dest = os.path.join(tmpdir, "copied.json")
    reports = []
    out_path, out_hdrs = urllib.request.urlretrieve(
        furl, dest, reporthook=lambda bn, bs, sz: reports.append((bn, bs, sz))
    )
    assert out_path == dest
    with open(dest, "rb") as f:
        assert f.read() == payload
    assert len(reports) >= 2
    assert reports[0][0] == 0 and reports[0][2] == len(payload)
    assert reports[-1][0] >= 1

    # Missing file raises URLError
    missing_url = _make_file_url(os.path.join(tmpdir, "no_such_file.txt"))
    try:
        urllib.request.urlopen(missing_url)
        assert False, "expected URLError for missing file"
    except urllib.error.URLError:
        pass

# Unknown scheme raises URLError
try:
    urllib.request.urlopen("unknownscheme://example.test/x")
    assert False, "expected URLError for unknown scheme"
except urllib.error.URLError as err:
    assert "unknown url type: unknownscheme" in str(err)

print("file_and_unknown_ok")

# ---------------------------------------------------------------------------
# 8. Custom OpenerDirector / BaseHandler / build_opener / install_opener
# ---------------------------------------------------------------------------
order_log = []


class EarlyHandler(urllib.request.BaseHandler):
    handler_order = 100

    def mock_request(self, req):
        order_log.append("early_req")
        req.add_header("X-Early", "yes")
        return req


class MockSchemeHandler(urllib.request.BaseHandler):
    handler_order = 500

    def mock_open(self, req):
        order_log.append("mock_open")
        body = (f"MOCK:{req.selector}:early={req.get_header('X-early')}").encode("utf-8")
        hdrs = {"Content-Type": "text/plain", "Content-Length": str(len(body))}
        return urllib.response.addinfourl(io.BytesIO(body), hdrs, req.full_url, code=200)

    def mock_response(self, req, resp):
        order_log.append("mock_resp")
        return resp


custom_opener = urllib.request.build_opener(MockSchemeHandler, EarlyHandler)
try:
    urllib.request.install_opener(custom_opener)
    with urllib.request.urlopen("mock://host/test") as resp:
        assert resp.status == 200
        assert resp.read() == b"MOCK:/test:early=yes"
    assert order_log == ["early_req", "mock_open", "mock_resp"]
finally:
    urllib.request.install_opener(None)

print("custom_opener_ok")

# ---------------------------------------------------------------------------
# 9. Loopback HTTP server via subprocess: GET, POST, PUT, custom headers,
#    301/302/307 redirects, redirect loop detection, 404 HTTPError, urlretrieve
# ---------------------------------------------------------------------------
_SERVER_CODE = r'''
import http.server
import sys

class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format, *args):
        pass

    def _send(self, code, body, headers=None, reason=None):
        self.send_response(code, reason)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        if headers:
            for k, v in headers.items():
                self.send_header(k, v)
        self.end_headers()
        if body:
            self.wfile.write(body)

    def do_GET(self):
        if self.path == "/__shutdown":
            self._send(200, b"bye")
            self.server._done = True
            return
        if self.path.startswith("/hello"):
            x_hdr = self.headers.get("X-Test", "")
            ua = self.headers.get("User-Agent", "")
            body = f"GET:{self.path}:x={x_hdr}:ua={ua.startswith('Python-urllib/')}".encode("utf-8")
            self._send(200, body, {"Content-Type": "text/plain", "X-Reply": "ok"})
            return
        if self.path == "/redirect302":
            self._send(302, b"moving", {"Location": "/final"})
            return
        if self.path == "/redirect301":
            self._send(301, b"moved", {"Location": "/final"})
            return
        if self.path == "/redirect307":
            self._send(307, b"temp", {"Location": "/final"})
            return
        if self.path == "/final":
            x_hdr = self.headers.get("X-Keep", "")
            unred = self.headers.get("X-Drop", "")
            body = f"FINAL:keep={x_hdr}:drop={unred}".encode("utf-8")
            self._send(200, body, {"Content-Type": "text/plain"})
            return
        if self.path == "/loop":
            self._send(302, b"again", {"Location": "/loop"})
            return
        if self.path == "/notfound":
            self._send(404, b"missing resource", {"Content-Type": "text/plain", "X-Err": "yes"})
            return
        if self.path == "/download":
            self._send(200, b"0123456789" * 20, {"Content-Type": "application/octet-stream"})
            return
        self._send(400, b"unknown")

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        data = self.rfile.read(length) if length > 0 else b""
        ct = self.headers.get("Content-Type", "")
        body = f"POST:{self.path}:ct={ct}:data=".encode("utf-8") + data
        self._send(201, body, {"Content-Type": "text/plain"})

    def do_PUT(self):
        length = int(self.headers.get("Content-Length", "0"))
        data = self.rfile.read(length) if length > 0 else b""
        body = b"PUT:" + data
        self._send(200, body, {"Content-Type": "text/plain"})

with http.server.HTTPServer(("127.0.0.1", 0), Handler) as srv:
    srv._done = False
    port = srv.server_address[1]
    sys.stdout.write(f"READY {port}\n")
    sys.stdout.flush()
    while not srv._done:
        srv.handle_request()
'''

proc = subprocess.Popen(
    [sys.executable, "-c", _SERVER_CODE],
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
)
try:
    ready_line = proc.stdout.readline().decode("ascii").strip()
    assert ready_line.startswith("READY "), f"unexpected server line: {ready_line!r}"
    port = int(ready_line.split()[1])
    base_url = f"http://127.0.0.1:{port}"

    # 9a. GET with custom header
    req_get = urllib.request.Request(
        f"{base_url}/hello?a=1", headers={"X-Test": "pyrust"}
    )
    with urllib.request.urlopen(req_get, timeout=5) as resp:
        assert resp.status == 200
        assert resp.code == 200
        assert resp.getcode() == 200
        assert resp.geturl() == f"{base_url}/hello?a=1"
        assert resp.info()["X-Reply"] == "ok"
        assert resp.headers["Content-Type"] == "text/plain"
        assert resp.read() == b"GET:/hello?a=1:x=pyrust:ua=True"

    # 9b. POST with form-encoded bytes data
    form_bytes = urllib.parse.urlencode({"k": "v", "n": "42"}).encode("utf-8")
    with urllib.request.urlopen(f"{base_url}/submit", data=form_bytes, timeout=5) as resp:
        assert resp.status == 201
        assert resp.read() == b"POST:/submit:ct=application/x-www-form-urlencoded:data=" + form_bytes

    # 9c. POST with str data raises TypeError
    try:
        urllib.request.urlopen(f"{base_url}/submit", data="not-bytes", timeout=5)
        assert False, "expected TypeError for str POST data"
    except TypeError:
        pass

    # 9d. Explicit PUT method
    req_put = urllib.request.Request(
        f"{base_url}/item", data=b"raw-body", method="PUT"
    )
    with urllib.request.urlopen(req_put, timeout=5) as resp:
        assert resp.status == 200
        assert resp.read() == b"PUT:raw-body"

    # 9e. 302 redirect preserves regular headers & drops unredirected headers
    req_r = urllib.request.Request(
        f"{base_url}/redirect302", headers={"X-Keep": "kept"}
    )
    req_r.add_unredirected_header("X-Drop", "secret")
    with urllib.request.urlopen(req_r, timeout=5) as resp:
        assert resp.status == 200
        assert resp.geturl() == f"{base_url}/final"
        assert resp.read() == b"FINAL:keep=kept:drop="

    # 9f. 301 and 307 redirects
    for path in ("/redirect301", "/redirect307"):
        with urllib.request.urlopen(f"{base_url}{path}", timeout=5) as resp:
            assert resp.status == 200
            assert resp.geturl() == f"{base_url}/final"
            assert resp.read() == b"FINAL:keep=:drop="

    # 9g. Redirect loop detection
    try:
        urllib.request.urlopen(f"{base_url}/loop", timeout=5)
        assert False, "expected HTTPError for redirect loop"
    except urllib.error.HTTPError as err:
        assert err.code == 302
        assert "infinite loop" in err.reason
        err.close()

    # 9h. 404 raises HTTPError which acts as a readable response
    try:
        urllib.request.urlopen(f"{base_url}/notfound", timeout=5)
        assert False, "expected HTTPError 404"
    except urllib.error.HTTPError as err:
        assert err.code == 404
        assert err.status == 404
        assert err.getcode() == 404
        assert err.reason == "Not Found"
        assert err.headers["X-Err"] == "yes"
        assert err.geturl() == f"{base_url}/notfound"
        with err as e:
            assert e.read() == b"missing resource"

    # 9i. urlretrieve over HTTP with reporthook
    with tempfile.TemporaryDirectory() as tmpdir:
        dl_dest = os.path.join(tmpdir, "dl.bin")
        hook_events = []
        fname, dl_h = urllib.request.urlretrieve(
            f"{base_url}/download",
            dl_dest,
            reporthook=lambda bn, bs, sz: hook_events.append((bn, bs, sz)),
        )
        assert fname == dl_dest
        assert dl_h["Content-Length"] == "200"
        with open(dl_dest, "rb") as f:
            assert f.read() == b"0123456789" * 20
        assert hook_events[0] == (0, 8192, 200)
        assert hook_events[-1][0] >= 1

    # 9j. Clean shutdown
    with urllib.request.urlopen(f"{base_url}/__shutdown", timeout=5) as resp:
        assert resp.read() == b"bye"
finally:
    try:
        proc.wait(timeout=10)
    except Exception:
        proc.kill()
        proc.wait(timeout=5)
    if proc.stdout:
        proc.stdout.close()
    if proc.stderr:
        proc.stderr.close()

assert proc.returncode == 0
print("http_loopback_ok")
print("urllib_request_all_ok")
