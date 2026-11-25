import http
import http.cookiejar as cj
import os
import subprocess
import sys
import tempfile
import urllib
import urllib.request as ureq
import urllib.robotparser as rp

# ---------------------------------------------------------------------------
# 1. Module & package identity
# ---------------------------------------------------------------------------
assert http.cookiejar is sys.modules["http.cookiejar"]
assert urllib.robotparser is sys.modules["urllib.robotparser"]
assert issubclass(cj.LoadError, OSError)
assert "CookieJar" in cj.__all__
assert "MozillaCookieJar" in cj.__all__
assert "LWPCookieJar" in cj.__all__
assert "RobotFileParser" in rp.__all__
print("imports_ok")

# ---------------------------------------------------------------------------
# 2. http.cookiejar: Cookie attributes, nonstandard attrs, expiration
# ---------------------------------------------------------------------------
c1 = cj.Cookie(
    version=0,
    name="session_id",
    value="abc123",
    port=None,
    port_specified=False,
    domain=".example.com",
    domain_specified=True,
    domain_initial_dot=True,
    path="/app",
    path_specified=True,
    secure=True,
    expires=2000000000,
    discard=False,
    comment="auth cookie",
    comment_url="https://example.com/cookies",
    rest={"HttpOnly": None, "SameSite": "Lax"},
    rfc2109=False,
)
assert c1.version == 0
assert c1.name == "session_id"
assert c1.value == "abc123"
assert c1.domain == ".example.com"
assert c1.domain_specified is True
assert c1.domain_initial_dot is True
assert c1.path == "/app"
assert c1.path_specified is True
assert c1.secure is True
assert c1.expires == 2000000000
assert c1.discard is False
assert c1.comment == "auth cookie"
assert c1.comment_url == "https://example.com/cookies"
assert c1.rfc2109 is False
assert c1.has_nonstandard_attr("HttpOnly") is True
assert c1.get_nonstandard_attr("HttpOnly") is None
assert c1.get_nonstandard_attr("SameSite") == "Lax"
assert c1.get_nonstandard_attr("Missing", "fallback") == "fallback"
c1.set_nonstandard_attr("Priority", "High")
assert c1.has_nonstandard_attr("Priority") is True
assert c1.get_nonstandard_attr("Priority") == "High"
assert c1.is_expired(1999999999) is False
assert c1.is_expired(2000000000) is True
assert c1.is_expired(2000000001) is True
assert str(c1) == "<Cookie session_id=abc123 for .example.com/app>"
print("cookie_attrs_ok")

# ---------------------------------------------------------------------------
# 3. DefaultCookiePolicy & CookieJar extract / add_cookie_header / clear
# ---------------------------------------------------------------------------
policy = cj.DefaultCookiePolicy(blocked_domains=["blocked.example"])
assert policy.is_blocked("blocked.example") is True
assert policy.is_blocked("allowed.example") is False
assert policy.netscape is True
assert policy.rfc2965 is False
policy.set_allowed_domains(["example.com", ".example.com"])
assert policy.is_not_allowed("example.com") is False
assert policy.is_not_allowed("other.org") is True
policy.set_allowed_domains(None)
assert policy.is_not_allowed("other.org") is False


class _FakeHeaders:
    def __init__(self, items):
        self._items = items

    def get_all(self, name, failobj=None):
        low = name.lower()
        vals = [v for k, v in self._items if k.lower() == low]
        return vals if vals else failobj


class _FakeResponse:
    def __init__(self, headers):
        self._headers = _FakeHeaders(headers)

    def info(self):
        return self._headers


jar = cj.CookieJar()
req = ureq.Request("https://www.example.com/app/dashboard")
resp = _FakeResponse(
    [
        (
            "Set-Cookie",
            "sid=xyz; Path=/app; Domain=.example.com; Secure; HttpOnly; Expires=Wed, 18 May 2033 03:33:20 GMT",
        ),
        ("Set-Cookie", "temp=1; Path=/app"),
        ("Set-Cookie", "rootc=r; Path=/"),
        ("Set-Cookie", "old=gone; Path=/; Expires=Thu, 01 Jan 1970 00:00:01 GMT"),
    ]
)
jar.extract_cookies(resp, req)
# Expired 'old' cookie is not retained on extract_cookies (matching CPython)
assert len(jar) == 3
names_in_jar = sorted(c.name for c in jar)
assert names_in_jar == ["rootc", "sid", "temp"]

# Manually insert an expired cookie to verify add_cookie_header skips it & clear_expired_cookies removes it
expired_cookie = cj.Cookie(
    version=0,
    name="old",
    value="gone",
    port=None,
    port_specified=False,
    domain="www.example.com",
    domain_specified=False,
    domain_initial_dot=False,
    path="/",
    path_specified=True,
    secure=False,
    expires=1,
    discard=False,
    comment=None,
    comment_url=None,
    rest={},
)
jar.set_cookie(expired_cookie)
assert len(jar) == 4

# add_cookie_header on HTTPS /app/page includes sid, temp, rootc (excludes expired 'old')
req_https = ureq.Request("https://www.example.com/app/page")
jar.add_cookie_header(req_https)
hdr = req_https.get_header("Cookie")
assert hdr is not None
hdr_parts = set(hdr.split("; "))
assert hdr_parts == {"sid=xyz", "temp=1", "rootc=r"}

# add_cookie_header on HTTP omits Secure cookie 'sid'
req_http = ureq.Request("http://www.example.com/app/page")
jar.add_cookie_header(req_http)
hdr_http = req_http.get_header("Cookie")
assert set(hdr_http.split("; ")) == {"temp=1", "rootc=r"}

# clear_expired_cookies removes 'old'
jar.clear_expired_cookies()
assert sorted(c.name for c in jar) == ["rootc", "sid", "temp"]

# clear_session_cookies removes discard=True ('temp', 'rootc') and keeps 'sid'
jar.clear_session_cookies()
assert [c.name for c in jar] == ["sid"]

# clear(domain, path, name) + KeyError on missing
jar.clear(".example.com", "/app", "sid")
assert len(jar) == 0
try:
    jar.clear(".example.com", "/app", "sid")
    assert False, "expected KeyError"
except KeyError:
    pass
print("cookiejar_core_ok")

# ---------------------------------------------------------------------------
# 4. MozillaCookieJar & LWPCookieJar save / load / LoadError
# ---------------------------------------------------------------------------
with tempfile.TemporaryDirectory() as tmpdir:
    moz_path = os.path.join(tmpdir, "moz_cookies.txt")
    moz = cj.MozillaCookieJar(moz_path)
    moz.set_cookie(c1)
    moz.save(ignore_discard=True, ignore_expires=True)

    moz2 = cj.MozillaCookieJar(moz_path)
    moz2.load(ignore_discard=True, ignore_expires=True)
    assert len(moz2) == 1
    loaded_m = list(moz2)[0]
    assert loaded_m.name == "session_id"
    assert loaded_m.value == "abc123"
    assert loaded_m.domain == ".example.com"
    assert loaded_m.path == "/app"
    assert loaded_m.secure is True
    assert loaded_m.expires == 2000000000

    lwp_path = os.path.join(tmpdir, "lwp_cookies.txt")
    lwp = cj.LWPCookieJar(lwp_path)
    lwp.set_cookie(c1)
    lwp.save(ignore_discard=True, ignore_expires=True)

    lwp2 = cj.LWPCookieJar(lwp_path)
    lwp2.load(ignore_discard=True, ignore_expires=True)
    assert len(lwp2) == 1
    loaded_l = list(lwp2)[0]
    assert loaded_l.name == "session_id"
    assert loaded_l.value == "abc123"
    assert loaded_l.domain == ".example.com"
    assert loaded_l.path == "/app"
    assert loaded_l.secure is True
    assert loaded_l.expires == 2000000000
    assert loaded_l.comment == "auth cookie"

    # Bad magic header raises LoadError
    bad_path = os.path.join(tmpdir, "bad.txt")
    with open(bad_path, "w") as bf:
        bf.write("not a cookie file\n")
    for cls in (cj.MozillaCookieJar, cj.LWPCookieJar):
        try:
            cls(bad_path).load()
            assert False, f"expected LoadError for {cls.__name__}"
        except cj.LoadError:
            pass
print("file_cookiejars_ok")

# ---------------------------------------------------------------------------
# 5. urllib.request.HTTPCookieProcessor loopback test (subprocess 127.0.0.1:0)
# ---------------------------------------------------------------------------
_COOKIE_SERVER_CODE = '''
import http.server, sys

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass
    def do_GET(self):
        if self.path == "/set":
            self.send_response(200)
            self.send_header("Set-Cookie", "token=hello42; Path=/")
            self.send_header("Content-Length", "3")
            self.end_headers()
            self.wfile.write(b"set")
        elif self.path == "/echo":
            val = self.headers.get("Cookie", "").encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Length", str(len(val)))
            self.end_headers()
            self.wfile.write(val)
        elif self.path == "/robots.txt":
            body = b"User-agent: *\\nDisallow: /secret\\nAllow: /public\\nCrawl-delay: 5\\n"
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()

with http.server.HTTPServer(("127.0.0.1", 0), H) as srv:
    sys.stdout.write(f"READY {srv.server_address[1]}\\n")
    sys.stdout.flush()
    for _ in range(4):
        srv.handle_request()
'''

proc = subprocess.Popen(
    [sys.executable, "-c", _COOKIE_SERVER_CODE],
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
)
try:
    ready = proc.stdout.readline().decode("ascii").strip()
    assert ready.startswith("READY "), ready
    port = int(ready.split()[1])
    base = f"http://127.0.0.1:{port}"

    live_jar = cj.CookieJar()
    opener = ureq.build_opener(ureq.HTTPCookieProcessor(live_jar))
    with opener.open(f"{base}/set", timeout=5) as r1:
        assert r1.read() == b"set"
    assert len(live_jar) == 1
    with opener.open(f"{base}/echo", timeout=5) as r2:
        assert r2.read() == b"token=hello42"

    # Also test RobotFileParser.read() against the live loopback server!
    rfp_live = rp.RobotFileParser(f"{base}/robots.txt")
    rfp_live.read()
    assert rfp_live.mtime() > 0
    assert rfp_live.can_fetch("MyBot", f"{base}/public/page") is True
    assert rfp_live.can_fetch("MyBot", f"{base}/secret/data") is False
    assert rfp_live.crawl_delay("MyBot") == 5

    # 404 on robots.txt -> allow_all = True
    rfp_404 = rp.RobotFileParser(f"{base}/missing-robots.txt")
    rfp_404.read()
    assert rfp_404.allow_all is True
    assert rfp_404.can_fetch("MyBot", f"{base}/anything") is True
finally:
    if proc.poll() is None:
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
            proc.wait()
assert proc.returncode == 0
print("loopback_cookie_and_robots_ok")

# ---------------------------------------------------------------------------
# 6. urllib.robotparser offline parsing & CPython 3.12 parity
# ---------------------------------------------------------------------------
parser = rp.RobotFileParser()
assert parser.mtime() == 0
assert parser.can_fetch("*", "/anything") is False
assert parser.crawl_delay("*") is None
assert parser.request_rate("*") is None
assert parser.site_maps() is None

parser.set_url("https://example.com/robots.txt")
assert parser.url == "https://example.com/robots.txt"

robots_lines = [
    "# Global comment",
    "Sitemap: https://example.com/sitemap.xml",
    "",
    "User-agent: GoodBot",
    "Allow: /private/public",
    "Disallow: /private",
    "Crawl-delay: 2",
    "Request-rate: 3/10",
    "",
    "User-agent: BadBot",
    "Disallow: /",
    "",
    "User-agent: *",
    "Disallow: /admin/",
    "Disallow: /tmp",
    "Allow: /",
    "Crawl-delay: 7",
    "Sitemap: https://example.com/sitemap2.xml",
]
parser.parse(robots_lines)
assert parser.mtime() > 0
assert parser.can_fetch("GoodBot/1.0", "/private/public/index.html") is True
assert parser.can_fetch("GoodBot/1.0", "/private/secret") is False
assert parser.can_fetch("GoodBot/1.0", "/open") is True
assert parser.crawl_delay("GoodBot") == 2
rate = parser.request_rate("GoodBot")
assert rate == (3, 10)
assert rate.requests == 3
assert rate.seconds == 10

assert parser.can_fetch("BadBot", "/") is False
assert parser.can_fetch("BadBot", "/anything") is False
assert parser.crawl_delay("BadBot") is None
assert parser.request_rate("BadBot") is None

assert parser.can_fetch("OtherBot", "/admin/panel") is False
assert parser.can_fetch("OtherBot", "/tmp/file") is False
assert parser.can_fetch("OtherBot", "/blog") is True
assert parser.crawl_delay("OtherBot") == 7
assert parser.site_maps() == [
    "https://example.com/sitemap.xml",
    "https://example.com/sitemap2.xml",
]

# Empty Disallow means allow all
p_empty = rp.RobotFileParser()
p_empty.parse(["User-agent: *", "Disallow:"])
assert p_empty.can_fetch("AnyBot", "/secret/path") is True

# Manual disallow_all / allow_all flags
p_flags = rp.RobotFileParser()
p_flags.disallow_all = True
assert p_flags.can_fetch("*", "/any") is False
p_flags.disallow_all = False
p_flags.allow_all = True
assert p_flags.can_fetch("*", "/any") is True

print("robotparser_ok")
print("cookiejar_robotparser_all_ok")
