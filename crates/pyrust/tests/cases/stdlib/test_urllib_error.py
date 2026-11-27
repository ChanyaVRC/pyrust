# Dedicated CPython 3.12 parity tests for `urllib.error` (issue #3443):
# URLError, HTTPError, ContentTooShortError.

import email.message
import io
import re
import sys
import tempfile
import urllib
import urllib.error
import urllib.response
from urllib.error import ContentTooShortError, HTTPError, URLError


def attempt(fn, *args, **kwargs):
    try:
        return ("OK", fn(*args, **kwargs))
    except Exception as e:
        return (type(e).__name__, str(e))


def err_name(fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
        return "no error"
    except Exception as e:
        return type(e).__name__


def norm_repr(obj):
    text = repr(obj)
    text = re.sub(r" at \d+ ", " at <id> ", text)
    text = re.sub(r" at 0x[0-9a-fA-F]+", "", text)
    return text


# 1. Module metadata and wiring
err = urllib.error
print("name:", err.__name__)
print("in sys.modules:", sys.modules["urllib.error"] is err)
print("urllib.error attr:", urllib.error is err)
print("urllib.error via __import__:", __import__("urllib", fromlist=["error"]).error is err)
print("__all__:", err.__all__)
print("doc first line:", err.__doc__.splitlines()[0])
print("doc paragraphs:", len(err.__doc__.split("\n\n")))
print("io attr:", err.io is io)
print("urllib attr:", err.urllib is urllib)
print("response reachable:", err.urllib.response is urllib.response)
print("from-import:", [n for n in err.__all__ if getattr(err, n) is globals()[n]])
for name in err.__all__:
    cls = getattr(err, name)
    print(name, cls.__module__, cls.__qualname__, [c.__name__ for c in cls.__mro__])
    print("  bases:", [c.__name__ for c in cls.__bases__])
print("URLError.__doc__:", URLError.__doc__)
print("HTTPError.__doc__:", HTTPError.__doc__)
print("ContentTooShortError.__doc__:", ContentTooShortError.__doc__)
print("public names:", sorted(n for n in dir(err) if not n.startswith("_")))

# 2. Class relationships
print("URLError < OSError:", issubclass(URLError, OSError))
print("URLError < Exception:", issubclass(URLError, Exception))
print("HTTPError < URLError:", issubclass(HTTPError, URLError))
print("HTTPError < OSError:", issubclass(HTTPError, OSError))
print("HTTPError < addinfourl:", issubclass(HTTPError, urllib.response.addinfourl))
print("HTTPError < addbase:", issubclass(HTTPError, urllib.response.addbase))
print("HTTPError < _TemporaryFileWrapper:", issubclass(HTTPError, tempfile._TemporaryFileWrapper))
print("ContentTooShortError < URLError:", issubclass(ContentTooShortError, URLError))
print("ContentTooShortError < addinfourl:", issubclass(ContentTooShortError, urllib.response.addinfourl))
print("URLError < HTTPError:", issubclass(URLError, HTTPError))

# 3. URLError
e = URLError("no host given")
print("URLError args:", e.args)
print("URLError reason:", e.reason)
print("URLError str:", str(e))
print("URLError repr:", repr(e))
print("URLError filename:", e.filename)
print("URLError errno/strerror:", e.errno, e.strerror)
e2 = URLError("a", "b")
print("URLError 2 args:", e2.args, e2.errno, e2.strerror, e2.filename, e2.filename2)
print("URLError isinstance:", isinstance(e, OSError), isinstance(e, URLError), isinstance(e, Exception))
print("URLError has filename in dict:", "filename" in vars(e))
print("URLError vars keys:", sorted(vars(e)))

e = URLError("boom", "file.txt")
print("URLError+filename:", e.args, e.reason, e.filename, str(e), repr(e))
print("URLError+filename vars:", sorted(vars(e)))
e = URLError("boom", None)
print("URLError filename None:", e.filename, "filename" in vars(e))
e = URLError("boom", filename="kw.txt")
print("URLError filename kw:", e.filename)
e = URLError("boom", "")
print("URLError filename empty:", repr(e.filename), "filename" in vars(e))

# reason of various types
for reason in (OSError(2, "No such file"), 42, None, 3.5, b"bytes", ["a", "b"], {"k": "v"}, ("only",)):
    e = URLError(reason)
    print("reason", type(reason).__name__, "->", e.args == (reason,), e.reason is reason, str(e))
e = URLError(OSError(2, "No such file"))
print("nested reason:", e.reason.errno, e.reason.strerror, str(e))

# `'...%s' % self.reason` is a tuple-format in CPython: a tuple reason is spread
print("tuple reason (2):", attempt(str, URLError((1, 2))))
print("tuple reason (0):", attempt(str, URLError(())))
print("tuple reason (1):", attempt(str, URLError(("a",))))
e = URLError((1, 2))
print("tuple reason args:", e.args, e.reason)

# subclass behaviours
class MyURLError(URLError):
    pass


e = MyURLError("sub")
print("subclass:", e.args, str(e), repr(e), isinstance(e, URLError))


class Reasoned(URLError):
    def __init__(self, text):
        super().__init__("wrapped: " + text, "fn")


e = Reasoned("x")
print("super().__init__:", e.args, e.reason, e.filename, str(e))

# __init__ argument errors
print("no args:", attempt(URLError))
print("too many:", err_name(URLError, 1, 2, 3))
print("unexpected kw:", err_name(URLError, "x", foo=1))
print("reason kw:", attempt(lambda: str(URLError(reason="kwreason"))))

# raising and catching
for exc in (URLError("raised"), HTTPError("http://x/", 404, "Not Found", {}, None)):
    try:
        raise exc
    except OSError as caught:
        print("caught as OSError:", type(caught).__name__, caught is exc)
try:
    raise URLError("x")
except URLError as caught:
    print("traceback type:", type(caught.__traceback__).__name__)

# reassigning attributes
e = URLError("old")
e.reason = "new"
print("reassigned reason:", e.reason, e.args, str(e))
e.args = ("other",)
print("reassigned args:", e.args, e.reason, str(e))

# copy goes through OSError.__reduce__ (args + __dict__)
import copy

e = URLError("copyme", "fn")
print("copy:", attempt(lambda: copy.copy(e).reason))
e = ContentTooShortError("short", b"xx")
print("copy short:", attempt(lambda: copy.copy(e).content))

# 4. ContentTooShortError
e = ContentTooShortError("retrieval incomplete: got only 5 out of 10 bytes", ("x.bin", {}))
print("CTS args:", e.args)
print("CTS reason:", e.reason)
print("CTS content:", e.content)
print("CTS str:", str(e))
print("CTS repr:", repr(e))
print("CTS isinstance:", isinstance(e, URLError), isinstance(e, OSError))
print("CTS filename:", e.filename)
print("CTS vars:", sorted(vars(e)))
print("CTS no args:", attempt(ContentTooShortError))
print("CTS one arg:", err_name(ContentTooShortError, "m"))
print("CTS three args:", err_name(ContentTooShortError, "m", "c", "x"))
e = ContentTooShortError(message="kw", content=None)
print("CTS kw:", e.args, e.content)

# 5. HTTPError with fp=None
hdrs = email.message.Message()
hdrs["Content-Type"] = "text/plain"
e = HTTPError("http://example.com/a", 404, "Not Found", hdrs, None)
print("HTTPError code:", e.code)
print("HTTPError msg:", e.msg)
print("HTTPError hdrs:", e.hdrs is hdrs)
print("HTTPError headers:", e.headers is hdrs)
print("HTTPError fp:", type(e.fp).__name__)
print("HTTPError filename:", e.filename)
print("HTTPError reason:", e.reason)
print("HTTPError url:", e.url)
print("HTTPError status:", e.status)
print("HTTPError getcode:", e.getcode())
print("HTTPError geturl:", e.geturl())
print("HTTPError info:", e.info() is hdrs)
print("HTTPError args:", e.args)
print("HTTPError str:", str(e))
print("HTTPError repr:", repr(e))
print("HTTPError isinstance:", isinstance(e, OSError), isinstance(e, URLError), isinstance(e, urllib.response.addinfourl))
print("HTTPError vars keys:", sorted(vars(e)))
print("HTTPError file is BytesIO:", type(e.file).__name__)
print("HTTPError read:", e.read())
print("HTTPError read again:", e.read())
print("HTTPError readline:", e.readline())
print("HTTPError readable:", e.readable())
print("HTTPError closed:", e.closed)
e.close()
print("HTTPError closed after close:", e.closed)
print("HTTPError read after close:", attempt(e.read))
print("HTTPError content-type via headers:", e.headers["Content-Type"])

# 6. HTTPError with a file-like fp
body = io.BytesIO(b"line1\nline2\nline3\n")
e = HTTPError("http://example.com/b", 500, "Server Error", {"X": "y"}, body)
print("fp attr:", e.fp is body)
print("file attr:", e.file is body)
print("code/status/getcode:", e.code, e.status, e.getcode())
print("hdrs dict:", e.hdrs, e.headers, e.info())
print("readline:", e.readline())
print("read(3):", e.read(3))
print("readlines:", e.readlines())
print("read at end:", e.read())
print("str:", str(e))
print("repr:", repr(e))
print("isinstance addbase:", isinstance(e, urllib.response.addbase))
print("iteration:", [line for line in HTTPError("u", 1, "m", {}, io.BytesIO(b"a\nb\n"))])
print("next:", next(iter(HTTPError("u", 1, "m", {}, io.BytesIO(b"first\nsecond\n")))))
print("tell/seek:", e.tell(), e.seek(0), e.read(5))
print("seekable/readable/writable:", e.seekable(), e.readable(), e.writable())
print("fileno:", attempt(e.fileno))
e.close()
print("closed:", body.closed, e.fp.closed, e.closed)
print("read after close:", attempt(e.read))
e.close()
print("double close ok")

# context manager
body = io.BytesIO(b"ctx data")
e = HTTPError("http://example.com/c", 403, "Forbidden", {}, body)
with e as entered:
    print("with entered is self:", entered is e)
    print("with read:", entered.read())
print("with closed:", body.closed)
print("enter after close:", attempt(e.__enter__))

# with fp=None context manager
with HTTPError("http://example.com/d", 400, "Bad", {}, None) as e2:
    print("with None fp:", e2.read(), type(e2.fp).__name__ if e2.fp is not None else None)
print("closed after with:", e2.closed)

# raise inside with-statement
try:
    with HTTPError("u", 418, "I'm a teapot", {}, io.BytesIO(b"tea")) as e3:
        raise e3
except HTTPError as caught:
    print("raised inside with:", caught is e3, caught.code, caught.closed)

# 7. Properties
e = HTTPError("u", 301, "Moved", {"a": 1}, None)
print("reason property:", e.reason)
e.msg = "Changed"
print("reason follows msg:", e.reason, str(e), repr(e))
print("headers property:", e.headers)
e.headers = {"b": 2}
print("headers setter:", e.headers, e.hdrs, e.info())
e.hdrs = {"c": 3}
print("hdrs follows:", e.headers)
print("reason setter:", attempt(setattr, e, "reason", "x"))
print("reason deleter:", attempt(delattr, e, "reason"))
print("headers deleter:", attempt(delattr, e, "headers"))
print("property types:", type(HTTPError.reason).__name__, type(HTTPError.headers).__name__)
print("reason fget/fset:", HTTPError.reason.fget is not None, HTTPError.reason.fset)
print("headers fget/fset:", HTTPError.headers.fget is not None, HTTPError.headers.fset is not None)
print("reason in vars(e):", "reason" in vars(e), "headers" in vars(e))
print("class attrs:", sorted(n for n in vars(HTTPError) if not n.startswith("__") or n in ("__init__", "__str__", "__repr__")))
print("inherited reason on URLError:", "reason" in vars(URLError))

# 8. __str__ / __repr__ edge cases
print("str code types:", str(HTTPError("u", "404", "NF", {}, None)))
print("str none code:", str(HTTPError("u", None, None, None, None)))
print("repr none:", repr(HTTPError("u", None, None, None, None)))
print("repr msg quotes:", repr(HTTPError("u", 1, "it's", {}, None)))
print("repr msg bytes:", repr(HTTPError("u", 1, b"by", {}, None)))
print("repr msg unicode:", repr(HTTPError("u", 1, "é中", {}, None)))
print("str msg unicode:", str(HTTPError("u", 1, "é中", {}, None)))
print("str tuple msg:", str(HTTPError("u", 1, ("a", "b"), {}, None)))
print("repr tuple msg:", repr(HTTPError("u", 1, ("a", "b"), {}, None)))
print("str format:", "%s" % HTTPError("u", 404, "NF", {}, None), f"{HTTPError('u', 404, 'NF', {}, None)}")
print("str.format:", "{!r}".format(HTTPError("u", 404, "NF", {}, None)))


class Msg:
    def __str__(self):
        return "custom-str"

    def __repr__(self):
        return "custom-repr"


print("custom msg str:", str(HTTPError("u", 1, Msg(), {}, None)))
print("custom msg repr:", repr(HTTPError("u", 1, Msg(), {}, None)))
print("custom reason str:", str(URLError(Msg())))

# 9. HTTPError constructor edge cases
print("HTTPError no args:", attempt(HTTPError))
print("HTTPError 4 args:", err_name(HTTPError, "u", 1, "m", {}))
print("HTTPError 6 args:", err_name(HTTPError, "u", 1, "m", {}, None, 1))
print("HTTPError kw:", attempt(lambda: repr(HTTPError(url="u", code=2, msg="m", hdrs={}, fp=None))))
print("HTTPError bad kw:", err_name(HTTPError, "u", 1, "m", {}, None, extra=1))
e = HTTPError("u", 1, "m", {}, None)
print("HTTPError args empty:", e.args)
print("HTTPError errno:", e.errno, e.strerror)
print("HTTPError fp is falsy-but-not-None:", HTTPError("u", 1, "m", {}, io.BytesIO()).fp is not None)


class Falsy:
    def __bool__(self):
        return False

    def read(self, *a):
        return b"falsy"

    closed = False

    def close(self):
        pass


f = Falsy()
e = HTTPError("u", 1, "m", {}, f)
print("falsy fp kept:", e.fp is f, e.file is f)

# subclassing HTTPError
class MyHTTPError(HTTPError):
    def __str__(self):
        return "mine: " + HTTPError.__str__(self)


e = MyHTTPError("u", 502, "Bad Gateway", {}, None)
print("subclass str:", str(e))
print("subclass repr:", repr(e))
print("subclass reason:", e.reason)
print("subclass isinstance:", isinstance(e, HTTPError), isinstance(e, URLError), isinstance(e, urllib.response.addinfourl))

# HTTPError identity / hashing / equality semantics
e1 = HTTPError("u", 1, "m", {}, None)
e2 = HTTPError("u", 1, "m", {}, None)
print("eq:", e1 == e2, e1 == e1, hash(e1) == hash(e1))
print("bool:", bool(e1))

# 10. Exception handling idioms used with urllib.request
def fake_urlopen(kind):
    if kind == "http":
        raise HTTPError("http://h/", 404, "Not Found", {}, None)
    if kind == "url":
        raise URLError(OSError(111, "Connection refused"))
    if kind == "short":
        raise ContentTooShortError("short", b"")
    return "ok"


for kind in ("http", "url", "short", "ok"):
    try:
        res = fake_urlopen(kind)
    except HTTPError as e:
        print(kind, "-> HTTPError", e.code, e.reason)
    except ContentTooShortError as e:
        print(kind, "-> ContentTooShortError", e.content)
    except URLError as e:
        print(kind, "-> URLError", e.reason)
    else:
        print(kind, "->", res)

# exception chaining and traceback text
try:
    try:
        raise OSError(2, "inner")
    except OSError as inner:
        raise URLError(inner) from inner
except URLError as outer:
    print("cause:", outer.__cause__ is outer.reason, outer.__cause__.errno)

try:
    try:
        raise KeyError("k")
    except KeyError:
        raise HTTPError("u", 500, "chained", {}, None)
except HTTPError as outer:
    print("context:", type(outer.__context__).__name__, outer.__suppress_context__)

# 11. Pickle round trip
import pickle

for original in (URLError("p"), ContentTooShortError("m", b"c")):
    r = attempt(lambda o=original: type(pickle.loads(pickle.dumps(o))).__name__)
    print("pickle", type(original).__name__, r[0])

# 12. Interplay with urllib.response.addinfourl behaviour
e = HTTPError("http://example.com/i", 200, "OK", {"h": "v"}, io.BytesIO(b"xyz"))
print("addinfourl.info:", urllib.response.addinfourl.info(e))
print("addinfourl.geturl:", urllib.response.addinfourl.geturl(e))
print("addinfourl.getcode:", urllib.response.addinfourl.getcode(e))
print("addinfourl.status:", urllib.response.addinfourl.status.fget(e))
print("repr(addinfourl):", norm_repr(urllib.response.addinfourl(io.BytesIO(b""), {}, "u", 1))[:20])
print("addbase __repr__ not used:", repr(e).startswith("<HTTPError"))
print("closehook-free close:", attempt(e.close))
print("method resolution:", HTTPError.__str__ is not URLError.__str__, HTTPError.__init__ is not URLError.__init__)
print("getcode resolution:", HTTPError.getcode is urllib.response.addinfourl.getcode)
print("close resolution:", HTTPError.close is urllib.response.addbase.close)
print("__enter__ resolution:", HTTPError.__enter__ is urllib.response.addbase.__enter__)
print("super init alias:", HTTPError._HTTPError__super_init is urllib.response.addinfourl.__init__)
print("done")
