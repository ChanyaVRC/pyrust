# Parity fixture for the native `http.cookies` (issue #3478): output must match
# CPython 3.12.3 byte for byte.
import copy
import pickle
import time
import http.cookies as hc
import http
from http import cookies
from http.cookies import BaseCookie, CookieError, Morsel, SimpleCookie


def show(label, thunk):
    try:
        print(label, "->", repr(thunk()))
    except BaseException as exc:
        print(label, "!!", type(exc).__name__, exc)


# ---------------------------------------------------------------- metadata
print(hc.__all__)
print(hc.__name__, hc.CookieError.__module__, hc.Morsel.__module__)
print(CookieError.__mro__ == (CookieError, Exception, BaseException, object))
print(issubclass(Morsel, dict), issubclass(BaseCookie, dict), issubclass(SimpleCookie, BaseCookie))
print(hc._LegalChars)
print(hc._UnescapedChars)
print(hc._LegalKeyChars)
print(hc._LegalValueChars)
print(hc._weekdayname, hc._monthname)
print(len(hc._Translator), list(hc._Translator)[:6], hc._Translator[34], hc._Translator[92], hc._Translator[0], hc._Translator[255])
print(sorted(Morsel._reserved.items()))
print(sorted(Morsel._flags))
print(http.cookies is hc, "cookies" in dir(http))
print(hc.__doc__ == hc.__doc__.strip() or hc.__doc__.splitlines()[1])
print(Morsel.__doc__.splitlines()[0], BaseCookie.__doc__, SimpleCookie.__doc__.splitlines()[1])
print(isinstance(Morsel.key, property), isinstance(Morsel.value, property), isinstance(Morsel.coded_value, property))
print(Morsel.__str__ is Morsel.output, BaseCookie.__str__ is BaseCookie.output)
print(Morsel[str])
print(sorted(n for n in dir(Morsel) if not n.startswith("__")))
print(sorted(n for n in dir(BaseCookie) if not n.startswith("__")))
print(sorted(n for n in dir(SimpleCookie) if not n.startswith("__")))

# ----------------------------------------------------------- _quote/_unquote
for text in ["", "abc", "a b", "a;b", 'a"b', "a\\b", "a\nb", "a\x7fb", "é", "日本", "a,b", "a=b", "a[b]", "!#$%&'*+-.^_`|~:", "()/<=>?@{}", "\x00\x1f", "\xff", "Ā"]:
    quoted = hc._quote(text)
    print(repr(text), repr(quoted), repr(hc._unquote(quoted)))
print(hc._quote(None), hc._unquote(None))
show("quote int", lambda: hc._quote(5))
show("quote bytes", lambda: hc._quote(b"abc"))
show("quote noargs", lambda: hc._quote())
show("quote kw", lambda: hc._quote(str="a b"))
show("quote extra", lambda: hc._quote("a", "b"))
show("unquote noargs", lambda: hc._unquote())
show("unquote int", lambda: hc._unquote(5))
for text in ['', '"', 'a', '"a', 'a"', '""', '"a"', r'"\012"', r'"\0123"', r'"\400"', r'"\377"', r'"\18"', r'"\\012"', r'"\""', r'"a\"b"', '"a\\\nb"', '"a\\', r'"\\"', r'"\\\""', '"\\é"', r'"\a\b"', r'"\1"', r'"\12"', r'"\123\456"', '"x\\\\\\012"', r'"\9"']:
    print(repr(text), repr(hc._unquote(text)))
show("is_legal ok", lambda: bool(hc._is_legal_key("abc")))
show("is_legal bad", lambda: hc._is_legal_key("a b"))
show("is_legal empty", lambda: hc._is_legal_key(""))
show("is_legal int", lambda: hc._is_legal_key(1))

# ---------------------------------------------------------------- _getdate
real_time = time.time
time.time = lambda: 1_000_000_000.5
try:
    print(hc._getdate())
    print(hc._getdate(0))
    print(hc._getdate(3600))
    print(hc._getdate(-86400 * 365))
    print(hc._getdate(2.5))
    print(hc._getdate(future=10, weekdayname=list("ABCDEFG"), monthname=[None] + list("abcdefghijkl")))
    show("getdate str", lambda: hc._getdate("x"))
    show("getdate bad kw", lambda: hc._getdate(foo=1))
    show("getdate too many", lambda: hc._getdate(1, 2, 3, 4))
    m = Morsel()
    m.set("k", "v", "v")
    m["expires"] = 86400
    print(m.OutputString())
    m["expires"] = True
    print(m.OutputString())
    m["expires"] = 0
    print(m.OutputString())
    m["expires"] = "Wed, 09 Jun 2021 10:18:14 GMT"
    print(m.OutputString())
    m["expires"] = 1.5
    print(m.OutputString())
finally:
    time.time = real_time

# ------------------------------------------------------------------ Morsel
m = Morsel()
print(m.key, m.value, m.coded_value, dict(m))
print(list(m))
print(repr(m), str(m), m.output())
for name in ("expires", "path", "comment", "domain", "max-age", "secure", "httponly", "version", "samesite"):
    print(name, repr(m[name]), m.isReservedKey(name), m.isReservedKey(name.upper()), m.isReservedKey(name.title()))
print(m.isReservedKey("nope"))
show("isReserved int", lambda: m.isReservedKey(1))
show("getitem bad", lambda: m["bogus"])
show("setitem bad", lambda: m.__setitem__("bogus", 1))


def setitem_bad():
    m["Bogus-Attr"] = 1


show("setitem bad2", setitem_bad)
show("setitem int key", lambda: m.__setitem__(1, 1))
show("setitem args", lambda: m.__setitem__("path"))
show("init args", lambda: Morsel(1))
show("setdefault bad", lambda: m.setdefault("zzz"))
show("setdefault ok", lambda: m.setdefault("PATH", "/x"))
show("setdefault new", lambda: m.setdefault("Comment"))
m["Comment"] = ""
m["Path"] = ""

show("set reserved", lambda: m.set("Path", "v", "v"))
show("set reserved2", lambda: m.set("EXPIRES", "v", "v"))
for bad in ["", "a b", "a;b", 'a"b', "a\x00b", "é", "a,b", "a/b", "a=b", "[a]", "a\nb"]:
    show("set key %r" % bad, lambda: m.set(bad, "v", "v"))
show("set int", lambda: m.set(1, "v", "v"))
show("set args", lambda: m.set("a", "b"))
for legal in ["a", "A1", "!#$%&'*+-.^_`|~:", "x" * 50]:
    show("set legal %r" % legal, lambda: m.set(legal, "v", "v"))
    print(m.key == legal)

m = Morsel()
m.set("session", "abc123", "abc123")
print(m.key, m.value, m.coded_value)
m["Path"] = "/app"
m["Domain"] = "example.com"
m["Max-Age"] = 3600
m["Secure"] = True
m["HttpOnly"] = True
m["SameSite"] = "Strict"
m["Comment"] = "hello world"
m["Version"] = 1
print(repr(m.OutputString()))
print(repr(m.output()), repr(str(m)), repr(m))
print(repr(m.output(header="Cookie:")), repr(m.output(None, "X:")))
print(repr(m.OutputString(attrs=["path", "secure"])))
print(repr(m.OutputString(attrs=("max-age",))))
print(repr(m.OutputString(attrs=set())))
print(repr(m.OutputString(attrs={"version": 1, "samesite": 2})))
print(repr(m.OutputString(attrs="path")))
show("OutputString int attrs", lambda: m.OutputString(attrs=5))
print(repr(m.output(attrs=["domain"], header="Z:")))
print(repr(m.js_output()))
print(repr(m.js_output(["path"])))
m2 = Morsel()
m2.set("q", 'a"b', '"a\\"b"')
print(repr(m2.js_output()), repr(m2.output()))
show("output args", lambda: m.output(1, 2, 3))
show("output kw", lambda: m.output(foo=1))
show("OutputString args", lambda: m.OutputString(1, 2))

# values of different types in attributes
m3 = Morsel()
m3.set("k", 1, 1)
m3["max-age"] = 5
print(m3.OutputString())
m3["max-age"] = True
print(m3.OutputString())
m3["max-age"] = False
print(m3.OutputString())
m3["max-age"] = 0
print(m3.OutputString())
m3["max-age"] = "60"
print(m3.OutputString())
m3["max-age"] = 1.5
print(m3.OutputString())
m3["max-age"] = -5
print(m3.OutputString())
m3["max-age"] = 10**30
print(m3.OutputString())
m3["max-age"] = ""
m3["comment"] = "a b;c"
print(m3.OutputString())
m3["comment"] = 'q"u\\o'
print(m3.OutputString())
m3["comment"] = "simple"
print(m3.OutputString())
m3["comment"] = 5
print(m3.OutputString())
m3["comment"] = b"x"
print(m3.OutputString())
m3["comment"] = None
print(m3.OutputString())
m3["comment"] = ""
m3["secure"] = 1
m3["httponly"] = "yes"
print(m3.OutputString())
m3["secure"] = 0
m3["httponly"] = []
print(m3.OutputString())
m3["secure"] = None
print(m3.OutputString())
m3["secure"] = False
m3["domain"] = "d"
m3["path"] = None
m3["version"] = 0
m3["samesite"] = ["a"]
print(m3.OutputString())


class WithStr:
    def __str__(self):
        return "STR"

    def __repr__(self):
        return "REPR"

    def __eq__(self, other):
        return False

    __hash__ = None


m3["path"] = WithStr()
print(m3.OutputString())
m3.set(WithStr().__str__(), WithStr(), WithStr())
print(m3.OutputString(), "|", repr(m3.value), repr(m3.coded_value))

# update / copy / eq / setdefault / state
a = Morsel()
a.set("k", "v", "v")
a.update({"PATH": "/p", "Domain": "d"})
print(sorted((k, v) for k, v in a.items() if v != ""))
show("update bad", lambda: a.update({"bogus": 1}))
show("update partial", lambda: a.update({"path": "/zzz", "bogus": 1}))
print(a["path"])
show("update pairs", lambda: a.update([("secure", True), ("Version", "1")]))
print(a["secure"], a["version"])
show("update kw", lambda: a.update(path="/kw"))
show("update none", lambda: a.update(None))
show("update int", lambda: a.update(5))
show("update noargs", lambda: a.update())
show("update int key", lambda: a.update({1: 2}))
show("update morsel", lambda: a.update(Morsel()))
show("update after", lambda: a.update(a))
b = a.copy()
print(type(b).__name__, b == a, b is not a, b.key, b.value, b.coded_value, vars(b) == vars(a))
b["path"] = "/changed"
print(a == b, a != b, b == a, b != a)
print(a == 1, a != 1, a == {}, a.__eq__(1), a.__ne__(1))
c = Morsel()
c.set("k", "v", "v")
print(a == c, c == c.copy())
d = c.copy()
d.set("k", "v2", "v")
print(c == d)
d.set("k", "v", "v2")
print(c == d)
d.set("k2", "v", "v")
print(c == d)
show("eq args", lambda: a.__eq__())
show("copy args", lambda: a.copy(1))
show("hash", lambda: hash(a))
print(a.__getstate__())
e = Morsel()
show("setstate", lambda: e.__setstate__({"key": "K", "value": "V", "coded_value": "C"}))
print(e.key, e.value, e.coded_value)
show("setstate bad", lambda: e.__setstate__({"key": "K"}))
show("setstate int", lambda: e.__setstate__(1))
print(copy.copy(a) == a, copy.deepcopy(a) == a, type(copy.copy(a)).__name__)
for proto in range(pickle.HIGHEST_PROTOCOL + 1):
    r = pickle.loads(pickle.dumps(a, proto))
    print(proto, r == a, r.key, r.value, r.coded_value, dict(r) == dict(a))


class MySubMorsel(Morsel):
    pass


sm = MySubMorsel()
sm.set("k", "v", "v")
print(repr(sm), str(sm), type(sm.copy()).__name__, sm == sm.copy(), sm.copy() == sm)
print(type(sm).__name__, sm.output(), sm.js_output() == Morsel.js_output(sm))


class OutputOverride(Morsel):
    def OutputString(self, attrs=None):
        return "OVERRIDDEN"


oo = OutputOverride()
print(oo.output(), repr(oo), str(oo), repr(oo.js_output()))


class NoInit(Morsel):
    def __init__(self):
        pass


ni = NoInit()
print(dict(ni), ni.copy().get("path"))
show("noinit key", lambda: ni.key)
show("noinit repr", lambda: repr(ni))
show("noinit setitem", lambda: ni.__setitem__("path", "/x"))
print(dict(ni))

# --------------------------------------------------------------- BaseCookie
C = BaseCookie()
print(repr(C), str(C), repr(C.output()), repr(C.js_output()), len(C))
print(BaseCookie({}), BaseCookie(None), BaseCookie(""), BaseCookie([]), BaseCookie(0))
C = BaseCookie({"a": 1, "b": "x y", "c": None, "d": 1.5, "e": [1, 2], "f": b"q"})
print(repr(C))
print(repr(C.output()))
print({k: (m.key, m.value, m.coded_value) for k, m in C.items()})
print(C["a"].value == "1", type(C["a"]).__name__, list(C))
C["z"] = 5
C["a"] = "new"
print(repr(C), list(C))
C["m"] = Morsel()
print(list(C), repr(C["m"]), C["m"].key)
show("output with unset morsel", lambda: C.output())
del C["m"]
show("setitem bad key", lambda: C.__setitem__("bad key", 1))
show("setitem reserved", lambda: C.__setitem__("Path", 1))
show("setitem int key", lambda: C.__setitem__(1, 1))
show("setitem empty key", lambda: C.__setitem__("", 1))
show("setitem args", lambda: C.__setitem__("a"))
print(sorted(C))
show("init two", lambda: BaseCookie(1, 2))
show("init kw", lambda: BaseCookie(input="a=b"))
print(repr(BaseCookie(input="a=b")))
show("init int", lambda: BaseCookie(5))
show("init list", lambda: BaseCookie([("a", "b")]))
show("init pairs", lambda: BaseCookie([["a", "b"]]))
show("init set", lambda: BaseCookie({"a"}))
show("init tuple", lambda: BaseCookie(("a=b",)))
print(repr(BaseCookie("x=1; y=2")), repr(BaseCookie({"x": 1}.items().__class__ and {"q": 2})))

C = BaseCookie()
C["k"] = "v"
C["k"]["path"] = "/"
C["k"]["Secure"] = True
C["j"] = "w x"
C["j"]["max-age"] = 5
print(repr(C.output()))
print(repr(C.output(sep="|")))
print(repr(C.output(sep="")))
print(repr(C.output(header="H:")))
print(repr(C.output(["path"])))
print(repr(C.output(attrs=["secure"], header="Cookie:", sep="\n")))
print(repr(C.output(None, "Set-Cookie:", ", ")))
print(repr(str(C)), repr(C))
print(repr(C.js_output()))
print(repr(C.js_output(["path"])))
show("output bad sep", lambda: C.output(sep=1))
show("output sep none", lambda: C.output(sep=None))
show("output args", lambda: C.output(1, 2, 3, 4))
show("output kw", lambda: C.output(foo=1))
show("js_output args", lambda: C.js_output(1, 2))
show("value_decode", lambda: C.value_decode("a b"))
show("value_encode", lambda: C.value_encode("a b"))
show("value_encode int", lambda: C.value_encode(5))
show("value_encode none", lambda: C.value_encode(None))
show("value_decode args", lambda: C.value_decode())
show("load none", lambda: C.load(None))
show("load int", lambda: C.load(5))
show("load list", lambda: C.load([1]))
show("load bytes", lambda: C.load(b"a=b"))
show("load dict", lambda: C.load({"x": "y"}))
show("load self", lambda: C.load(C))
show("load args", lambda: C.load())
print(repr(C))
show("load string returns", lambda: C.load("a=b"))

# --------------------------------------------------------------- SimpleCookie
S = SimpleCookie()
S["a"] = "b c"
S["d"] = 'q"x'
S["e"] = "semi;colon"
S["f"] = "back\\slash"
S["g"] = "newline\n"
S["h"] = "é日"
S["i"] = ""
S["j"] = 7
S["k"] = "plain"
S["l"] = "comma,sep"
S["m"] = "\x7f\xff"
S["n"] = "a=b"
for key, morsel in S.items():
    print(key, repr(morsel.value), repr(morsel.coded_value))
print(repr(S.output()))
print(repr(S))
print(S.value_encode("a b"), S.value_encode(5), S.value_encode(""), S.value_decode('"a\\"b"'), S.value_decode("x"), S.value_decode(None))
show("simple encode none", lambda: S.value_encode(None))
show("simple decode int", lambda: S.value_decode(5))
show("simple encode args", lambda: S.value_encode())
raw = S.output(header="", sep="; ")
print(repr(raw))
roundtrip = SimpleCookie()
roundtrip.load(S.output(header="", sep="; ").replace("; ; ", "; ").strip())
print(sorted(roundtrip))
loaded = SimpleCookie(S.output(header="").replace("\r\n", ";"))
print({k: v.value for k, v in loaded.items()})
same = SimpleCookie()
for key, morsel in S.items():
    same[key] = morsel.value
print(same.output() == S.output())

# ---------------------------------------------------------------- load()
CASES = [
    "",
    " ",
    "a=b",
    "a=b;",
    "a=b; ",
    "a=b;c=d",
    "a=b; c=d",
    "a=b ;c=d",
    "a=b c=d",
    "  a=b  ;  c=d  ",
    "a=b\n",
    "a=b\nc=d",
    "a=b\r\nc=d",
    "a = b",
    "a =b",
    "a= b",
    "a=",
    "a=;",
    "a=; b=c",
    "=b",
    "a",
    "a;b",
    "a; b=c",
    "a=b; c",
    "a=b; c; d=e",
    '"a"=b',
    'a="b"',
    'a="b c"',
    'a="b;c"',
    'a="b\\"c"',
    'a="b\\\\"',
    'a="b\\012c"',
    'a="b\\\nc"',
    'a="unterminated',
    'a="unterminated; b=c',
    'a="b"c',
    'a="b" c',
    'a="b"; c="d"',
    'a=""',
    'a="" ',
    "a=b=c",
    "a=b==c",
    "a==b",
    "a=b,c",
    "a=b;;c=d",
    "a=b; ;c=d",
    "a=b;\tc=d",
    "a=b\tc=d",
    "a=b\x0bc=d",
    "a=b\x0cc=d",
    "a=[b]",
    "a=[b",
    "a]=b",
    "[a]=b",
    "a=b[c]d",
    "a=b\"c",
    "a=b'c",
    "a=b(c)",
    "a=<b>",
    "a={b}",
    "a=b?c",
    "a=b@c",
    "a=b:c",
    "a=b/c",
    "a=b$c",
    "a=b%20c",
    "a=b\\c",
    "a=b é",
    "a=é",
    "é=a",
    "a=b c",
    "a =b",
    "k٠=v",
    "a=٠",
    "a=b; Path=/",
    "a=b; path=/",
    "a=b; PATH=/",
    "Path=/; a=b",
    "path=/",
    "a=b; Path",
    "a=b; Path=",
    "a=b; Path=; c=d",
    "a=b; Secure",
    "a=b; secure",
    "a=b; SECURE",
    "a=b; Secure=",
    "a=b; Secure=1",
    "a=b; Secure=0",
    "a=b; HttpOnly",
    "a=b; httponly=true",
    "a=b; Secure; HttpOnly",
    "a=b; Secure; HttpOnly; Path=/; Domain=x.com; Max-Age=10; Version=1; Comment=hi; SameSite=Lax",
    "a=b; Max-Age=10",
    "a=b; max-age=10",
    "a=b; Max-Age",
    "a=b; Version=1",
    "a=b; Comment=hello",
    'a=b; Comment="hello world"',
    'a=b; Comment="hel\\"lo"',
    'a=b; Comment="a\\012b"',
    "a=b; Domain=example.com",
    "a=b; Domain=.example.com",
    "a=b; SameSite=Strict",
    "a=b; SameSite=None",
    "a=b; expires=Wed, 09 Jun 2021 10:18:14 GMT",
    "a=b; Expires=Wed, 09 Jun 2021 10:18:14 GMT; Path=/",
    "a=b; expires=Wed, 09-Jun-2021 10:18:14 GMT",
    "a=b; expires=Wed, 09-Jun-21 10:18:14 GMT",
    "a=b; expires=Wednesday, 09-Jun-21 10:18:14 GMT",
    "a=b; expires=Wed, 9 Jun 2021 10:18:14 GMT",
    "a=b; expires=Wed, 09 Jun 2021 10:18:14 GMT; Secure",
    "a=b; expires=Wed, 09 Jun 2021 10:18:14 UTC",
    "a=b; expires=Wed, 09 Jun 2021 10:18:14",
    "a=b; expires=Wed 09 Jun 2021 10:18:14 GMT",
    "a=b; expires=Wed,09 Jun 2021 10:18:14 GMT",
    "a=b; expires=Wed,  09 Jun 2021 10:18:14 GMT",
    "a=b; expires=Wed, 09 Jun 2021 10:18:14  GMT",
    "a=b; expires=Wed, 09 Jun 2021 10:18:14 GMT ; Path=/x",
    "a=b; expires=Wed, 09 Jun 2021 10:18:14 GMTx",
    "a=b; expires=Wed, 09 Jun 2021 10:18:14 GMT;",
    "a=b; expires=Thu, 01 Jan 1970 00:00:00 GMT",
    "a=b; expires=Thu, 01-Jan-1970 00:00:00 GMT",
    "a=b; expires=Thursday, 01-Jan-70 00:00:00 GMT",
    "a=b; expires=Mon, 01 Jan 2024 00:00:00 GMT; c=d",
    "a=b; expires=",
    "a=b; expires",
    "a=b; expires=x",
    "expires=Wed, 09 Jun 2021 10:18:14 GMT; a=b",
    "a=Wed, 09 Jun 2021 10:18:14 GMT",
    "a=Wed, 09 Jun 2021 10:18:14 GMT; b=c",
    "a=Wed, 09 Jun 2021 10:18:14 GMTzzz",
    "a=Wed,",
    "a=Wed, b",
    "a=Wed, b=c",
    "a=1, 2",
    "a=1,2",
    "a=b; $Path=/",
    "$Version=1; a=b",
    "$Version=1; a=b; $Path=/; $Domain=x.com",
    '$Version="1"; a=b; $Path="/p"',
    "$Version=1; a=b; c=d; $Path=/c",
    "$Version=1; a=b; $Foo=1",
    "$Version=1; a=b; $Foo",
    "$Version=1; a=b; $Secure",
    "$Version=1; a=b; $",
    "$a=b",
    "$a",
    "$",
    "$$a=b",
    "a=b; $Path",
    "a=b; $Path=",
    "a=b; $path=/lower",
    "a=b; $Max-Age=5",
    "a=b; $Secure=1",
    "a=b; $Secure=",
    "a=b; $Expires=Wed, 09 Jun 2021 10:18:14 GMT",
    "a=b; Foo=bar; Path=/",
    "a=b; Path=/; Foo=bar",
    "Version=1; a=b",
    "a=b; Version=1; c=d; Path=/",
    "a=b; a=c",
    "a=b; A=c",
    "a=b; a=c; Path=/",
    "a=1; b=2; c=3; d=4; e=5",
    "a=b;c=d;e=f",
    "a=b; c=d; e",
    "a=b; c=d; Secure; e=f; HttpOnly",
    "a=b; c=d; Secure; e=f; Bogus",
    "a=b; c d=e",
    "a b=c",
    "a=b c",
    "a=b c; d=e",
    "a=b  c=d",
    "a=b ; ; ; c=d",
    "; a=b",
    ";a=b",
    ";",
    ";;",
    " ; ",
    "a=b;;",
    "a=;b",
    "a=;;b=c",
    "a=b; =c",
    "a=b; =",
    "a=b; c=",
    "a=b; c=;",
    "a=b; c= ;",
    "a=b; c= ; d=e",
    "a=b;c=d;Path=/;Domain=d",
    "a=b;Secure",
    "a=b ;Secure",
    "a=b ; Secure ; Path=/ ;",
    "a=b\tSecure",
    "a=b\nSecure",
    "a=b\nSecure\n",
    "a=b\n\n",
    "a=b \n",
    "\na=b",
    "\n\na=b\n\n",
    "\ta=b",
    "a=b;\n",
    "a=b;\n\n",
    "a=b\nPath=/",
    "a=b\r\nPath=/\r\n",
    "a-b=c",
    "a.b=c",
    "a_b=c",
    "a+b=c",
    "a*b=c",
    "a!b=c",
    "a#b=c",
    "a%b=c",
    "a&b=c",
    "a'b=c",
    "a~b=c",
    "a`b=c",
    "a>b=c",
    "a<b=c",
    "a@b=c",
    "a,b=c",
    "a:b=c",
    "a/b=c",
    "a$b=c",
    "a^b=c",
    "a|b=c",
    "a)b=c",
    "a(b=c",
    "a?b=c",
    "a}b=c",
    "a{b=c",
    "a=b=",
    "a===",
    "===",
    "=",
    "==a",
    "a\"b=c",
    "a;b=c",
    "a\\b=c",
    "a[b=c",
    "a b",
    "a b c",
    "key=" + "v" * 200,
    "k" * 200 + "=v",
    "a=b; " * 20,
    "a=b; c=d" * 3,
    'a="x"y="z"',
    'a="x";y="z"',
    'a="x" ;y="z"',
    'a="x\\"y" z=1',
    'a="x\\"; z=1',
    'a="\\',
    'a="\\"',
    'a="\\""',
    'a="',
    'a="\n"',
    'a="\r\n"; b=1',
    'a=" "',
    'a="   "',
    'a="; "',
    'a="="',
    'a="\\\\"; b=2',
    'a="\\\\\\"; b=2',
    'a="\\\\\\""; b=2',
    'a=\\"b',
    'a=b\\',
    "a=b%",
    "a=\x00",
    "\x00=a",
    "a=b\x01",
    "a=b\x7f",
    "a=b c",
    "a=b c",
    "a=b\u0085c",
    "a=b﻿",
    "a=b\U0001f600",
    "\U0001f600=a",
    "ａ=b",
    "a=ｂ",
    "Ⅷ=1",
    "٣=1",
    "a=٣",
    "İ=1",
    "ſ=1",
    "K=1",
    "a=b; K",
    "a=b; İ",
    "a=b; Pȧth=/",
    "a=b; ſecure",
    "a=b; Secure; ſecure",
]


def dump(cookie):
    return [(k, m.key, m.value, m.coded_value, {a: v for a, v in m.items() if v != ""}) for k, m in cookie.items()]


for text in CASES:
    for cls in (SimpleCookie, BaseCookie):
        cookie = cls()
        try:
            cookie.load(text)
            print(cls.__name__, repr(text), "=>", dump(cookie))
        except BaseException as exc:
            print(cls.__name__, repr(text), "!!", type(exc).__name__, exc, dump(cookie))

# load into existing cookie, overwriting
cookie = SimpleCookie("a=1; Path=/p")
cookie.load("a=2; Domain=d")
print(dump(cookie))
cookie.load("b=3")
cookie.load("garbage=\"")
print(dump(cookie))
cookie.load("a=4; Secure; bad bad bad")
print(dump(cookie))
cookie.load("Path=/")
print(dump(cookie))
cookie = SimpleCookie()
cookie.load("a=1; Path=/p")
morsel = cookie["a"]
cookie.load("a=2")
print(cookie["a"] is morsel, dump(cookie))

# a failed parse leaves the cookie untouched; value_decode runs during parse
calls = []


class Recording(SimpleCookie):
    def value_decode(self, val):
        calls.append(val)
        return SimpleCookie.value_decode(self, val)


rec = Recording()
rec.load("a=1; b=2; Path=/; c=3; Secure; garbage")
print(calls, dump(rec))
calls.clear()
rec2 = Recording()
rec2.load("a=1; b=2; Secure; Bogus; c=3")
print(calls, dump(rec2))
calls.clear()
rec3 = Recording()
show("rec3 load", lambda: rec3.load("a=1; $Foo=2; c=3"))
print(calls, dump(rec3))

# ------------------------------------------------- custom subclasses / hooks
class Upper(BaseCookie):
    def value_encode(self, val):
        s = str(val).upper()
        return s, "<%s>" % s

    def value_decode(self, val):
        return val.strip("<>").lower(), val


U = Upper()
U["a"] = "hello"
U["b"] = 12
print(repr(U), repr(U.output()), U["a"].value, U["a"].coded_value)
U.load("c=<WORLD>; d=plain")
print(dump(U))
U2 = Upper({"x": "y"})
print(repr(U2), repr(U2.output()))
U3 = Upper("p=<Q>")
print(repr(U3), dump(U3))


class BadEncode(BaseCookie):
    def value_encode(self, val):
        return val


be = BadEncode()
show("bad encode str", lambda: be.__setitem__("a", "xy"))
show("bad encode one", lambda: be.__setitem__("a", ["x"]))
show("bad encode three", lambda: be.__setitem__("a", ["x", "y", "z"]))
show("bad encode pair", lambda: be.__setitem__("a", ["x", "y"]))
print(dump(be))


class BadDecode(BaseCookie):
    def value_decode(self, val):
        return val


bd = BadDecode()
show("bad decode", lambda: bd.load("a=1"))
show("bad decode 2", lambda: bd.load("a=12"))
print(dump(bd))


class RaisingDecode(BaseCookie):
    def value_decode(self, val):
        raise RuntimeError("decode " + val)


show("raising decode", lambda: RaisingDecode("a=b"))
show("raising decode empty", lambda: RaisingDecode(""))


class Meta(BaseCookie):
    def __init__(self, *args, **kwargs):
        self.created = True
        super().__init__(*args, **kwargs)


mt = Meta("a=b")
print(mt.created, dump(mt), vars(mt))


class Setter(SimpleCookie):
    def __setitem__(self, key, value):
        calls.append(("set", key))
        super().__setitem__(key, value)


calls.clear()
st = Setter()
st["a"] = "1"
st.load({"b": "2", "c": 3})
st.load("d=4")
print(calls, sorted(st))
calls.clear()
st.update({"e": "5"})
print(calls, dict(st).keys())

# cookies hold Morsel copies through dict methods
src = SimpleCookie("a=1")
dst = SimpleCookie()
dst["a"] = src["a"]
print(dst["a"] is src["a"], dump(dst))
dst2 = SimpleCookie()
dst2.update(src)
print(dst2["a"] is src["a"], dump(dst2))
dst3 = SimpleCookie(src)
print(dump(dst3), dst3["a"] is src["a"])
dst4 = SimpleCookie({"a": src["a"]})
print(dump(dst4), dst4["a"] is src["a"])
print(copy.copy(src) == src, copy.deepcopy(src) == src, type(copy.deepcopy(src)).__name__)
print(src.copy().__class__.__name__, src == SimpleCookie("a=1"), src == BaseCookie("a=1"))

# pickling
full = SimpleCookie()
full["x"] = "a b"
full["x"]["path"] = "/"
full["x"]["secure"] = True
full["y"] = 5
for proto in range(pickle.HIGHEST_PROTOCOL + 1):
    r = pickle.loads(pickle.dumps(full, proto))
    print(proto, type(r).__name__, r == full, r.output() == full.output(), repr(r))
r = pickle.loads(pickle.dumps(Upper("c=<A>"), 2))
print(type(r).__name__, dump(r))

# error type / hierarchy
show("CookieError()", lambda: (_ for _ in ()).throw(CookieError("boom")))
try:
    SimpleCookie()["bad key"] = 1
except CookieError as exc:
    print(type(exc).__name__, exc, exc.args, type(exc).__module__, type(exc).__qualname__, repr(exc))
try:
    Morsel()["nope"] = 1
except Exception as exc:
    print(type(exc).__name__, exc, exc.args, isinstance(exc, CookieError))
print(CookieError.__doc__, CookieError.__bases__)
e = CookieError("a", "b")
print(e.args, str(e), repr(e))

# generic alias, subclasses and keys
print(Morsel[int], hasattr(BaseCookie, "__class_getitem__"))
print(Morsel[int, str])
show("cgi noarg", lambda: Morsel.__class_getitem__())
show("cgi two", lambda: Morsel.__class_getitem__(1, 2))
print(issubclass(Morsel, dict), Morsel.__mro__ == (Morsel, dict, object))
print(isinstance(SimpleCookie(), dict), SimpleCookie.__mro__ == (SimpleCookie, BaseCookie, dict, object))
print(Morsel.__init__.__qualname__ if hasattr(Morsel.__init__, "__qualname__") else "?")
print(SimpleCookie.__name__, SimpleCookie.__qualname__, SimpleCookie.__module__)

# examples from the module docstring
C = SimpleCookie()
C["fig"] = "newton"
C["sugar"] = "wafer"
print(repr(str(C)))
C = SimpleCookie()
C["rocky"] = "road"
C["rocky"]["path"] = "/cookie"
print(C.output(header="Cookie:"))
print(C.output(attrs=[], header="Cookie:"))
C = SimpleCookie()
C["rocky"] = "road"
C["rocky"]["path"] = "/cookie"
print(C.js_output())
C = SimpleCookie()
C.load("chips=ahoy; vienna=finger")
print(repr(str(C)))
C = SimpleCookie()
C.load('keebler="E=everybody; L=\\"Loves\\"; fudge=\\012;";')
print(repr(str(C)))
print(C["keebler"].value, "|", C["keebler"].coded_value)
C = SimpleCookie()
C["oreo"] = "doublestuff"
C["oreo"]["path"] = "/"
print(repr(str(C)))
C = SimpleCookie()
C["twix"] = "none for you"
print(C["twix"].value)
C = SimpleCookie()
C["number"] = 7
C["string"] = "seven"
print(C["number"].value, C["string"].value)
print(repr(C.output()))
