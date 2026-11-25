from http import cookies
from http.cookies import CookieError, BaseCookie, SimpleCookie, Morsel

assert cookies.__all__ == ["CookieError", "BaseCookie", "SimpleCookie"]
assert issubclass(CookieError, Exception)

# 1. Morsel basics
m = Morsel()
assert m.key is None and m.value is None and m.coded_value is None
for attr in ("expires", "path", "comment", "domain", "max-age", "secure", "httponly", "version", "samesite"):
    assert m.isReservedKey(attr)
    assert m.isReservedKey(attr.upper())
    assert m[attr] == ""
assert not m.isReservedKey("unknown")

m.set("session", "abc123", "abc123")
assert m.key == "session"
assert m.value == "abc123"
assert m.coded_value == "abc123"
m["Path"] = "/app"
m["Domain"] = "example.com"
m["Max-Age"] = 3600
m["Secure"] = True
m["HttpOnly"] = True
m["SameSite"] = "Strict"
m["Comment"] = "hello world"
m["Version"] = 1

assert m["path"] == "/app"
assert m["max-age"] == 3600
assert m.OutputString() == (
    'session=abc123; Comment="hello world"; Domain=example.com; HttpOnly; '
    'Max-Age=3600; Path=/app; SameSite=Strict; Secure; Version=1'
)
assert m.output() == "Set-Cookie: " + m.OutputString()
assert str(m) == m.output()
assert repr(m) == f"<Morsel: {m.OutputString()}>"
assert m.output(attrs=["path", "secure"], header="Cookie:") == "Cookie: session=abc123; Path=/app; Secure"
assert "document.cookie =" in m.js_output()
assert "session=abc123" in m.js_output()

# Morsel copy, equality, update, setdefault, state
m2 = m.copy()
assert m2 == m
assert not (m2 != m)
m2["path"] = "/other"
assert m2 != m
m2.update({"Path": "/app"})
assert m2 == m
assert m2.setdefault("path", "/ignored") == "/app"

state = m.__getstate__()
assert state == {"key": "session", "value": "abc123", "coded_value": "abc123"}
m3 = Morsel()
m3.__setstate__(state)
assert m3.key == "session" and m3.value == "abc123" and m3.coded_value == "abc123"

# Morsel error cases
for bad_attr in ("bogus", "foo-bar"):
    try:
        m[bad_attr] = "x"
        assert False
    except CookieError:
        pass

for bad_key in ("path", "Secure", "bad key", "semi;colon", "has\r\nnewline"):
    try:
        Morsel().set(bad_key, "v", "v")
        assert False
    except CookieError:
        pass

print("morsel_ok")

# 2. SimpleCookie creation, quoting, and output
c = SimpleCookie()
c["fig"] = "newton"
c["sugar"] = "wafer"
c["number"] = 7
c["quoted"] = 'hello; "world"\\test'
c["fig"]["path"] = "/"
c["fig"]["httponly"] = True
c["fig"]["samesite"] = "Lax"

assert len(c) == 4
assert "fig" in c and "missing" not in c
assert c["fig"].value == "newton"
assert c["number"].value == "7"
assert c["quoted"].value == 'hello; "world"\\test'
assert c["quoted"].coded_value == '"hello\\073 \\"world\\"\\\\test"'

print("simple_output:", repr(c.output()))
print("simple_repr:", repr(c))
assert c.output(attrs=[], header="Cookie:", sep="; ") == (
    'Cookie: fig=newton; Cookie: number=7; '
    'Cookie: quoted="hello\\073 \\"world\\"\\\\test"; Cookie: sugar=wafer'
)
assert "document.cookie" in c.js_output()

# 3. SimpleCookie.load() from string & dict
c2 = SimpleCookie()
c2.load("chips=ahoy; vienna=finger")
assert c2["chips"].value == "ahoy"
assert c2["vienna"].value == "finger"
assert c2.output() == "Set-Cookie: chips=ahoy\r\nSet-Cookie: vienna=finger"

# Complex quoted values and attributes
c3 = SimpleCookie()
c3.load('keebler="E=everybody; L=\\"Loves\\"; fudge=;"; Path=/bakery; Secure; HttpOnly; SameSite=None')
assert c3["keebler"].value == 'E=everybody; L="Loves"; fudge=;'
assert c3["keebler"]["path"] == "/bakery"
assert c3["keebler"]["secure"] is True
assert c3["keebler"]["httponly"] is True
assert c3["keebler"]["samesite"] == "None"

# Expires date parsing
c4 = SimpleCookie()
c4.load("id=42; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Max-Age=60; Domain=.example.org")
assert c4["id"].value == "42"
assert c4["id"]["expires"] == "Wed, 21 Oct 2015 07:28:00 GMT"
assert c4["id"]["max-age"] == "60"
assert c4["id"]["domain"] == ".example.org"

# $Version and $Path attributes
c5 = SimpleCookie("$Version=1; Customer=WILE_E_COYOTE; $Path=/acme")
assert c5["Customer"].value == "WILE_E_COYOTE"
assert c5["Customer"]["path"] == "/acme"

# Octal unquoting roundtrip
c6 = SimpleCookie()
c6.load(r'oct="a\073b\054c"')
assert c6["oct"].value == "a;b,c"

# Loading from dict
c7 = SimpleCookie({"alpha": "1", "beta": "two words"})
assert c7["alpha"].value == "1"
assert c7["beta"].value == "two words"
assert c7["beta"].coded_value == '"two words"'

# Invalid cookie strings should not partially populate
c_bad = SimpleCookie()
c_bad.load("Path=/only-attr")
assert len(c_bad) == 0

c_bad2 = SimpleCookie()
c_bad2.load("good=1; novalue")
assert len(c_bad2) == 0

# BaseCookie (raw encode/decode without quoting)
bc = BaseCookie()
bc["raw"] = "a b"
assert bc["raw"].value == "a b"
assert bc["raw"].coded_value == "a b"
bc.load("x=1; y=2")
assert bc["x"].value == "1" and bc["y"].value == "2"

# Custom subclass overriding value_encode/value_decode
class UpperCookie(BaseCookie):
    def value_encode(self, val):
        s = str(val)
        return s, s.upper()

    def value_decode(self, val):
        return val.lower(), val

uc = UpperCookie()
uc["k"] = "hello"
assert uc["k"].value == "hello"
assert uc["k"].coded_value == "HELLO"
uc.load("other=WORLD")
assert uc["other"].value == "world"
assert uc["other"].coded_value == "WORLD"

print("cookies_ok")
