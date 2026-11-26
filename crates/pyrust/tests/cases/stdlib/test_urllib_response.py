# Dedicated CPython 3.12 parity tests for `urllib.response` (issue #3347):
# addbase, addclosehook, addinfo, addinfourl.

import io
import re
import sys
import tempfile
import urllib.error
import urllib.response as resp


def attempt(fn, *args, **kwargs):
    try:
        return ("OK", fn(*args, **kwargs))
    except Exception as e:
        return (type(e).__name__, str(e))


def norm_repr(obj):
    # Object ids / addresses are not comparable across interpreters.
    text = repr(obj)
    text = re.sub(r" at \d+ ", " at <id> ", text)
    text = re.sub(r" at 0x[0-9a-fA-F]+", "", text)
    return text


# 1. Module metadata
print("module name:", resp.__name__)
print("in sys.modules:", sys.modules["urllib.response"] is resp)
print("__all__:", resp.__all__)
print("doc:", resp.__doc__.splitlines()[0])
print("tempfile attr:", resp.tempfile is tempfile)
for name in resp.__all__:
    cls = getattr(resp, name)
    print(name, cls.__module__, cls.__qualname__, [c.__name__ for c in cls.__mro__])
print("addbase.__doc__:", resp.addbase.__doc__)
print("addclosehook.__doc__:", resp.addclosehook.__doc__)
print("addinfo.__doc__:", resp.addinfo.__doc__)
print("addinfourl.__doc__:", resp.addinfourl.__doc__)
print("wrapper base:", resp.addbase.__bases__ == (tempfile._TemporaryFileWrapper,))
print("issubclass wrapper:", issubclass(resp.addinfourl, tempfile._TemporaryFileWrapper))
print("from-import:", [n for n in resp.__all__ if n in dir(resp)])

# 2. addbase attributes and delegation
raw = io.BytesIO(b"line1\nline2\nline3\n")
base = resp.addbase(raw)
print("file is fp:", base.file is raw, base.fp is raw)
print("name/delete:", base.name, base._closer.delete, base._closer.delete_on_close)
print("isinstance:", isinstance(base, tempfile._TemporaryFileWrapper), isinstance(base, resp.addbase))
print("repr:", norm_repr(base))
print("repr id matches:", ("at %d " % id(base)) in repr(base))
print("read(2):", base.read(2))
print("readline:", base.readline())
print("tell/seek:", base.tell(), base.seek(0), base.tell())
print("readable/seekable/writable:", base.readable(), base.seekable(), base.writable())
print("readlines:", base.readlines())
print("closed before:", base.closed, raw.closed)
print("fileno:", attempt(base.fileno)[0])
base.seek(0)
print("iter:", list(base))
base.seek(0)
print("next:", attempt(next, base))
base.seek(0)
print("writelines:", base.writelines([b"AB", b"CD"]), base.getvalue()[:6])
print("missing attr:", attempt(getattr, base, "nope_not_here")[0])
base.close()
print("closed after:", base.closed, raw.closed)
base.close()
print("double close ok:", base.closed)
print("closer:", base._closer.close_called)

# 3. Context manager protocol
raw = io.BytesIO(b"abc")
with resp.addbase(raw) as b:
    print("enter returns self:", isinstance(b, resp.addbase), b.read())
print("closed by exit:", b.closed, raw.closed)
print("reenter closed:", attempt(b.__enter__))
print("reenter with:", attempt(lambda: b.__enter__()))

def with_closed():
    with b:
        pass

print("with closed:", attempt(with_closed))

raw = io.BytesIO(b"abc")
b = resp.addbase(raw)
print("__exit__ result:", b.__exit__(None, None, None), b.closed)

raw = io.BytesIO(b"abc")


def raise_inside():
    with resp.addbase(raw):
        raise KeyError("boom")


print("exception propagates:", attempt(raise_inside), raw.closed)

# 4. addclosehook
calls = []


def hook(*args):
    calls.append(args)


raw = io.BytesIO(b"hooked")
h = resp.addclosehook(raw, hook, "x", 42)
print("hook attrs:", h.closehook is hook, h.hookargs)
print("isinstance:", isinstance(h, resp.addbase), isinstance(h, resp.addclosehook))
print("repr:", norm_repr(h))
print("read:", h.read())
h.close()
print("hook calls after close:", calls, h.closehook, h.hookargs, h.closed)
h.close()
print("hook calls after 2nd close:", calls)

calls.clear()
raw = io.BytesIO(b"ctx")
with resp.addclosehook(raw, hook, 1) as h:
    print("ctx read:", h.read())
print("ctx hook calls:", calls, raw.closed)

h = resp.addclosehook(io.BytesIO(b"x"), None)
print("no hook:", h.hookargs, attempt(h.close), h.closed)
h = resp.addclosehook(io.BytesIO(b"x"), hook)
print("empty hookargs:", h.hookargs)
calls.clear()
h.close()
print("empty hookargs call:", calls)


def bad_hook(*args):
    raise RuntimeError("hook failed")


raw = io.BytesIO(b"x")
h = resp.addclosehook(raw, bad_hook, 1)
print("failing hook:", attempt(h.close), "file closed:", raw.closed, h.closehook)
print("failing hook again:", attempt(h.close))

# 5. addinfo
headers = {"Content-Type": "text/plain"}
raw = io.BytesIO(b"info")
i = resp.addinfo(raw, headers)
print("info():", i.info() is headers, i.headers is headers)
print("isinstance:", isinstance(i, resp.addbase), isinstance(i, resp.addinfo))
print("repr:", norm_repr(i))
print("read:", i.read())
i.close()
print("closed:", i.closed)
i.headers = "changed"
print("info after set:", i.info())

# 6. addinfourl
raw = io.BytesIO(b"body")
u = resp.addinfourl(raw, headers, "http://example.com/x", 200)
print("url attrs:", u.url, u.code, u.status, u.getcode(), u.geturl())
print("headers:", u.headers is headers, u.info() is headers)
print("isinstance:", [isinstance(u, c) for c in (resp.addbase, resp.addinfo, resp.addinfourl)])
print("repr:", norm_repr(u))
print("read:", u.read())
u2 = resp.addinfourl(io.BytesIO(b""), headers, "http://example.com/y")
print("default code:", u2.code, u2.status, u2.getcode())
u2.code = 301
print("status follows code:", u2.status, u2.getcode())
print("set status:", attempt(setattr, u2, "status", 1))
print("del status:", attempt(delattr, u2, "status"))
print("status property:", type(resp.addinfourl.status).__name__)
u2.url = "http://elsewhere/"
print("geturl follows url:", u2.geturl())

with resp.addinfourl(io.BytesIO(b"abc"), headers, "u", code=204) as ctx:
    print("ctx:", ctx.code, ctx.read(), ctx.geturl())
print("ctx closed:", ctx.closed)
u3 = resp.addinfourl(io.BytesIO(b""), None, None, None)
print("all none:", u3.headers, u3.url, u3.code, u3.status)
u4 = resp.addinfourl(fp=io.BytesIO(b"k"), headers=headers, url="ku", code=1)
print("kwargs:", u4.url, u4.code, u4.read())

# 7. Constructor argument errors (exception type only: the exact wording of
# the arity message for a native `__init__` is not part of this fixture)
def error_type(fn, *args, **kwargs):
    return attempt(fn, *args, **kwargs)[0]


print("addbase():", error_type(resp.addbase))
print("addbase(1,2):", error_type(resp.addbase, 1, 2))
print("addclosehook(fp):", error_type(resp.addclosehook, io.BytesIO()))
print("addinfo(fp):", error_type(resp.addinfo, io.BytesIO()))
print("addinfourl(fp, h):", error_type(resp.addinfourl, io.BytesIO(), headers))
print("addinfourl 5 args:", error_type(resp.addinfourl, io.BytesIO(), headers, "u", 1, 2))
print("addbase(fp=...):", error_type(lambda: resp.addbase(fp=io.BytesIO(b"k")).read()))

# 8. Subclassing
class MyResp(resp.addinfourl):
    def __init__(self, fp, extra):
        super().__init__(fp, {"k": "v"}, "http://sub/", 202)
        self.extra = extra

    def close(self):
        calls.append("sub-close")
        super().close()


calls.clear()
m = MyResp(io.BytesIO(b"sub"), "ex")
print("subclass:", m.extra, m.code, m.status, m.geturl(), m.info(), m.read())
print("subclass repr:", norm_repr(m))
m.close()
print("subclass close:", calls, m.closed)
with MyResp(io.BytesIO(b"s"), "e") as m2:
    pass
print("subclass ctx:", calls, m2.closed)

# 9. File-like objects other than BytesIO
text = io.StringIO("alpha\nbeta\n")
t = resp.addinfourl(text, headers, "text://x", 200)
print("text read:", t.readline(), t.read())
t.close()
print("text closed:", text.closed, t.closed)


class NoClosed:
    def __init__(self):
        self.did_close = False

    def read(self, n=-1):
        return "data"

    def close(self):
        self.did_close = True


nc = NoClosed()
n = resp.addbase(nc)
print("no-closed read:", n.read())
print("no-closed closed attr:", attempt(lambda: n.closed))
n.close()
print("no-closed did_close:", nc.did_close)

# 10. HTTPError is a response
err = urllib.error.HTTPError("http://e/", 404, "Not Found", {"h": 1}, io.BytesIO(b"err body"))
print("HTTPError:", issubclass(urllib.error.HTTPError, resp.addinfourl), isinstance(err, resp.addbase))
print("HTTPError attrs:", err.code, err.status, err.geturl(), err.getcode(), err.headers, err.info())
print("HTTPError read:", err.read(), err.reason, str(err), repr(err))
err.close()
print("HTTPError closed:", err.closed)
err2 = urllib.error.HTTPError("http://e/", 500, "Boom", {}, None)
print("HTTPError no fp:", err2.read(), err2.fp is None, err2.code)
with urllib.error.HTTPError("http://e/", 403, "Forbidden", {}, io.BytesIO(b"x")) as err3:
    print("HTTPError ctx:", err3.code, err3.read())
print("HTTPError mro:", [c.__name__ for c in urllib.error.HTTPError.__mro__])

print("urllib_response_ok")
