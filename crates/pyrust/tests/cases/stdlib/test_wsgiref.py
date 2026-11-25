import io
import subprocess
import sys
import urllib.request as ureq
import warnings
import wsgiref
import wsgiref.handlers as whandlers
import wsgiref.headers as wheaders
import wsgiref.simple_server as wserver
import wsgiref.types as wtypes
import wsgiref.util as wutil
import wsgiref.validate as wvalidate

# ---------------------------------------------------------------------------
# 1. Module identity & wsgiref.types exports
# ---------------------------------------------------------------------------
assert wsgiref.util is sys.modules["wsgiref.util"]
assert wsgiref.headers is sys.modules["wsgiref.headers"]
assert wsgiref.handlers is sys.modules["wsgiref.handlers"]
assert wsgiref.simple_server is sys.modules["wsgiref.simple_server"]
assert wsgiref.validate is sys.modules["wsgiref.validate"]
assert wsgiref.types is sys.modules["wsgiref.types"]

for name in wtypes.__all__:
    assert hasattr(wtypes, name), name
print("wsgiref_imports_ok")

# ---------------------------------------------------------------------------
# 2. wsgiref.util
# ---------------------------------------------------------------------------
assert wutil.guess_scheme({}) == "http"
assert wutil.guess_scheme({"HTTPS": "on"}) == "https"
assert wutil.guess_scheme({"HTTPS": "yes"}) == "https"
assert wutil.guess_scheme({"HTTPS": "1"}) == "https"
assert wutil.guess_scheme({"HTTPS": "off"}) == "http"

assert wutil.is_hop_by_hop("Connection") is True
assert wutil.is_hop_by_hop("transfer-encoding") is True
assert wutil.is_hop_by_hop("Keep-Alive") is True
assert wutil.is_hop_by_hop("Content-Type") is False
assert wutil.is_hop_by_hop("X-Custom") is False

env = {
    "wsgi.url_scheme": "http",
    "SERVER_NAME": "example.com",
    "SERVER_PORT": "8080",
    "SCRIPT_NAME": "/app",
    "PATH_INFO": "/users/42",
    "QUERY_STRING": "active=1",
}
assert wutil.application_uri(env) == "http://example.com:8080/app"
assert wutil.request_uri(env) == "http://example.com:8080/app/users/42?active=1"
assert wutil.request_uri(env, include_query=False) == "http://example.com:8080/app/users/42"

env_std = {
    "wsgi.url_scheme": "https",
    "SERVER_NAME": "secure.example",
    "SERVER_PORT": "443",
    "SCRIPT_NAME": "",
    "PATH_INFO": "/index.html",
}
assert wutil.application_uri(env_std) == "https://secure.example/"
assert wutil.request_uri(env_std) == "https://secure.example/index.html"

shift_env = {"SCRIPT_NAME": "/base", "PATH_INFO": "/a/b/c"}
assert wutil.shift_path_info(shift_env) == "a"
assert shift_env == {"SCRIPT_NAME": "/base/a", "PATH_INFO": "/b/c"}
assert wutil.shift_path_info(shift_env) == "b"
assert shift_env == {"SCRIPT_NAME": "/base/a/b", "PATH_INFO": "/c"}
assert wutil.shift_path_info(shift_env) == "c"
assert shift_env == {"SCRIPT_NAME": "/base/a/b/c", "PATH_INFO": ""}
assert wutil.shift_path_info(shift_env) is None

test_env = {}
wutil.setup_testing_defaults(test_env)
assert test_env["SERVER_NAME"] == "127.0.0.1"
assert test_env["SERVER_PORT"] == "80"
assert test_env["REQUEST_METHOD"] == "GET"
assert test_env["wsgi.version"] == (1, 0)
assert test_env["wsgi.url_scheme"] == "http"

fw = wutil.FileWrapper(io.BytesIO(b"abcdefghij"), blksize=4)
assert list(fw) == [b"abcd", b"efgh", b"ij"]
fw.close()
print("wsgiref_util_ok")

# ---------------------------------------------------------------------------
# 3. wsgiref.headers.Headers
# ---------------------------------------------------------------------------
h = wheaders.Headers([("Content-Type", "text/plain"), ("X-Multi", "1")])
assert len(h) == 2
assert "content-type" in h
assert "CONTENT-TYPE" in h
assert "missing" not in h
assert h["content-type"] == "text/plain"
assert h["missing"] is None
assert h.get("missing", "d") == "d"
h.add_header("X-Multi", "2")
assert h.get_all("x-multi") == ["1", "2"]
h.add_header("Content-Disposition", "attachment", filename="report.txt")
assert h["content-disposition"] == 'attachment; filename="report.txt"'
assert h.setdefault("X-New", "val") == "val"
assert h.setdefault("x-new", "other") == "val"
h["X-Multi"] = "replaced"
assert h.get_all("x-multi") == ["replaced"]
del h["x-multi"]
assert "X-Multi" not in h
assert h.keys() == ["Content-Type", "Content-Disposition", "X-New"]
assert h.values() == ["text/plain", 'attachment; filename="report.txt"', "val"]
assert h.items() == [
    ("Content-Type", "text/plain"),
    ("Content-Disposition", 'attachment; filename="report.txt"'),
    ("X-New", "val"),
]
h_simple = wheaders.Headers([("Content-Type", "text/plain")])
assert str(h_simple) == "Content-Type: text/plain\r\n\r\n"
assert bytes(h_simple) == b"Content-Type: text/plain\r\n\r\n"
print("wsgiref_headers_ok")

# ---------------------------------------------------------------------------
# 4. wsgiref.handlers (format_date_time, read_environ, SimpleHandler, BaseCGIHandler)
# ---------------------------------------------------------------------------
assert whandlers.format_date_time(0) == "Thu, 01 Jan 1970 00:00:00 GMT"
assert isinstance(whandlers.read_environ(), dict)


def _echo_app(environ, start_response):
    body = (
        f"{environ['REQUEST_METHOD']} {environ['PATH_INFO']}?{environ.get('QUERY_STRING', '')}"
    ).encode("utf-8")
    start_response(
        "200 OK",
        [("Content-Type", "text/plain"), ("X-App", "pyrust")],
    )
    return [body]


out_buf = io.BytesIO()
err_buf = io.StringIO()
sh = whandlers.SimpleHandler(
    io.BytesIO(b""),
    out_buf,
    err_buf,
    {
        "REQUEST_METHOD": "GET",
        "SERVER_NAME": "localhost",
        "SERVER_PORT": "8000",
        "SERVER_PROTOCOL": "HTTP/1.1",
        "PATH_INFO": "/hello",
        "QUERY_STRING": "k=v",
    },
)
sh.run(_echo_app)
raw_resp = out_buf.getvalue().decode("iso-8859-1")
assert raw_resp.startswith("HTTP/1.0 200 OK\r\n")
assert "Content-Type: text/plain\r\n" in raw_resp
assert "X-App: pyrust\r\n" in raw_resp
assert "Content-Length: 14\r\n" in raw_resp
assert raw_resp.endswith("\r\n\r\nGET /hello?k=v")

# BaseCGIHandler emits Status: instead of HTTP/1.0
cgi_buf = io.BytesIO()
cgi_h = whandlers.BaseCGIHandler(
    io.BytesIO(b""),
    cgi_buf,
    io.StringIO(),
    {
        "REQUEST_METHOD": "GET",
        "SERVER_NAME": "localhost",
        "SERVER_PORT": "80",
        "SERVER_PROTOCOL": "HTTP/1.1",
        "PATH_INFO": "/cgi",
    },
)
cgi_h.run(_echo_app)
cgi_out = cgi_buf.getvalue().decode("iso-8859-1")
assert cgi_out.startswith("Status: 200 OK\r\n")
assert cgi_out.endswith("\r\n\r\nGET /cgi?")
print("wsgiref_handlers_ok")

# ---------------------------------------------------------------------------
# 5. wsgiref.validate.validator & demo_app
# ---------------------------------------------------------------------------
validated_demo = wvalidate.validator(wserver.demo_app)
v_env = {
    "REQUEST_METHOD": "GET",
    "SERVER_NAME": "localhost",
    "SERVER_PORT": "80",
    "SERVER_PROTOCOL": "HTTP/1.1",
    "SCRIPT_NAME": "",
    "PATH_INFO": "/demo",
    "QUERY_STRING": "x=1",
}
wutil.setup_testing_defaults(v_env)
captured = {}


def _sr(status, headers, exc_info=None):
    captured["status"] = status
    captured["headers"] = headers
    return lambda data: None


it_demo = validated_demo(v_env, _sr)
try:
    chunks = list(it_demo)
finally:
    it_demo.close()
demo_body = b"".join(chunks).decode("utf-8")
assert captured["status"] == "200 OK"
assert demo_body.startswith("Hello world!\n\n")
assert "PATH_INFO = '/demo'\n" in demo_body

# Validator rejects bad status
def _bad_status_app(environ, start_response):
    start_response("200", [("Content-Type", "text/plain")])
    return [b"x"]


try:
    e2 = {"QUERY_STRING": ""}
    wutil.setup_testing_defaults(e2)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        it2 = wvalidate.validator(_bad_status_app)(e2, _sr)
        try:
            list(it2)
        finally:
            it2.close()
    assert False, "expected AssertionError for bad status"
except AssertionError:
    pass

# Validator rejects non-bytes yield
def _bad_body_app(environ, start_response):
    start_response("200 OK", [("Content-Type", "text/plain")])
    return ["not-bytes"]


try:
    e3 = {"QUERY_STRING": ""}
    wutil.setup_testing_defaults(e3)
    it3 = wvalidate.validator(_bad_body_app)(e3, _sr)
    try:
        list(it3)
    finally:
        it3.close()
    assert False, "expected AssertionError for str body"
except AssertionError:
    pass

# Validator rejects hop-by-hop headers
def _bad_hdr_app(environ, start_response):
    start_response(
        "200 OK",
        [("Content-Type", "text/plain"), ("Connection", "keep-alive")],
    )
    return [b"ok"]


try:
    e4 = {"QUERY_STRING": ""}
    wutil.setup_testing_defaults(e4)
    it4 = wvalidate.validator(_bad_hdr_app)(e4, _sr)
    try:
        list(it4)
    finally:
        it4.close()
    assert False, "expected AssertionError for hop-by-hop header"
except AssertionError:
    pass
print("wsgiref_validate_ok")

# ---------------------------------------------------------------------------
# 6. wsgiref.simple_server.make_server loopback (subprocess on 127.0.0.1:0)
# ---------------------------------------------------------------------------
assert "WSGIServer/" in wserver.software_version

_WSGI_SERVER_CODE = '''
import sys
import wsgiref.simple_server as ws
import wsgiref.validate as wv

class QuietWSGIHandler(ws.WSGIRequestHandler):
    def log_message(self, format, *args):
        pass

def app(environ, start_response):
    path = environ["PATH_INFO"]
    qs = environ.get("QUERY_STRING", "")
    method = environ["REQUEST_METHOD"]
    length = int(environ.get("CONTENT_LENGTH") or "0")
    req_body = environ["wsgi.input"].read(length) if length else b""
    if path == "/demo":
        return ws.demo_app(environ, start_response)
    payload = f"{method}:{path}:{qs}:{req_body.decode('utf-8')}".encode("utf-8")
    start_response(
        "200 OK",
        [("Content-Type", "text/plain; charset=utf-8"), ("X-WSGI", "native")],
    )
    return [payload]

srv = ws.make_server("127.0.0.1", 0, wv.validator(app), handler_class=QuietWSGIHandler)
try:
    port = srv.server_address[1]
    sys.stdout.write(f"READY {port}\\n")
    sys.stdout.flush()
    for _ in range(3):
        srv.handle_request()
finally:
    srv.server_close()
'''

proc = subprocess.Popen(
    [sys.executable, "-c", _WSGI_SERVER_CODE],
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
)
try:
    ready = proc.stdout.readline().decode("ascii").strip()
    assert ready.startswith("READY "), ready
    port = int(ready.split()[1])
    base = f"http://127.0.0.1:{port}"

    with ureq.urlopen(f"{base}/greet?who=world", timeout=5) as r1:
        assert r1.status == 200
        assert r1.headers["X-WSGI"] == "native"
        assert r1.read() == b"GET:/greet:who=world:"

    with ureq.urlopen(f"{base}/post", data=b"payload=42", timeout=5) as r2:
        assert r2.status == 200
        assert r2.read() == b"POST:/post::payload=42"

    with ureq.urlopen(f"{base}/demo", timeout=5) as r3:
        assert r3.status == 200
        body = r3.read().decode("utf-8")
        assert body.startswith("Hello world!\n\n")
        assert "PATH_INFO = '/demo'\n" in body
finally:
    if proc.poll() is None:
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
            proc.wait()
assert proc.returncode == 0
print("wsgiref_loopback_ok")
print("wsgiref_all_ok")
