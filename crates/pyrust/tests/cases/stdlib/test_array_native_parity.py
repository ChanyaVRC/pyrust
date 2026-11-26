# Issue #3389: `array` is implemented 100% natively in Rust.  This fixture pins
# the CPython 3.12.3 `Modules/arraymodule.c` behaviour the port reproduces:
# module/type surface, the staged item-conversion diagnostics, calling-convention
# argument errors, construction paths, the live iterator, buffer exports,
# slicing, rich comparison, pickling (`__reduce_ex__` / `_array_reconstructor`)
# and the `__sizeof__` over-allocation model.  Output is platform dependent
# (C `long` / `wchar_t` sizes) and is compared against the same platform's CPython.
import array
import collections.abc
import copy
import io
import pickle
import sys
import weakref

A = array.array


def t(label, fn):
    try:
        result = fn()
        print(label, "OK", repr(result).encode("ascii", "backslashreplace").decode())
    except BaseException as exc:
        print(label, type(exc).__name__, exc)


# ── 1. Module and type surface ──────────────────────────────────────────────
print("vars:", list(vars(array)))
print("doc:", array.__doc__.splitlines()[0])
print("type doc:", A.__doc__.splitlines()[0])
print("ArrayType:", array.ArrayType is A, repr(array.typecodes))
print("names:", A.__name__, A.__qualname__, A.__module__, repr(A))
rec = array._array_reconstructor
print("reconstructor:", rec.__name__, rec.__module__, rec.__qualname__)
print("public:", [n for n in dir(A) if not n.startswith("_")])
dunders = ["__add__", "__buffer__", "__class_getitem__", "__contains__", "__copy__",
           "__deepcopy__", "__delitem__", "__eq__", "__ge__", "__getitem__", "__gt__",
           "__iadd__", "__imul__", "__iter__", "__le__", "__len__", "__lt__", "__mul__",
           "__ne__", "__reduce_ex__", "__release_buffer__", "__repr__", "__rmul__",
           "__setitem__", "__sizeof__"]
print("dunders:", [n for n in dunders if n not in vars(A)])
print("hash is None:", A.__hash__ is None)
print("property docs:", A.typecode.__doc__, "/", A.itemsize.__doc__)
it_type = type(iter(A("i")))
print("iterator type:", it_type, it_type.__name__, it_type.__qualname__, it_type.__module__)
print("iterator hidden:", "arrayiterator" in vars(array))
print("abc:", isinstance(A("i"), collections.abc.MutableSequence),
      issubclass(A, collections.abc.Sequence), isinstance(iter(A("i")), collections.abc.Iterator))
print("generic alias:", A[int], A[int, str], type(A[int]).__name__)
print("weakref:", weakref.ref(A("i", [1]))() is None or True, hasattr(A("i"), "__weakref__"))
print("no instance dict:", hasattr(A("i"), "__dict__"))


# ── 2. Item conversion diagnostics ──────────────────────────────────────────
class BadIndex:
    def __index__(self):
        return "x"


class BadFloat:
    def __float__(self):
        return "x"


cases = [
    ("b", [127, -128, 128, -129, 40000, -40000, 1 << 40, 1 << 70, 1.5, "x", None]),
    ("B", [255, 256, -1, 1 << 40, 1 << 70]),
    ("h", [32767, 32768, -32769, 1 << 40, 1 << 70]),
    ("H", [65535, 65536, -1, 1 << 31, 1 << 40, 1 << 70]),
    ("i", [1 << 31, -(1 << 31) - 1, 1 << 40, 1 << 70]),
    ("I", [(1 << 32) - 1, 1 << 32, -1, 1 << 40, 1 << 64, 1 << 70]),
    ("l", [1 << 31, 1 << 63, -(1 << 63) - 1, 1 << 70]),
    ("L", [1 << 32, -1, 1 << 64, 1 << 70]),
    ("q", [(1 << 63) - 1, 1 << 63, -(1 << 63) - 1, 1 << 70]),
    ("Q", [(1 << 64) - 1, 1 << 64, -1, 1 << 70]),
    ("f", [1, True, 1e300, "x", None, 1 << 2000]),
    ("d", [1, "x", None, 1 << 2000]),
    ("u", ["a", 65, "ab", "", b"a", "\U0001F600"]),
]
for tc, values in cases:
    for value in values:
        t("enc %s %s" % (tc, ascii(value)[:24]), lambda: A(tc, [value]).tolist())
t("bad __index__", lambda: A("i", [BadIndex()]))
t("bad __index__ I", lambda: A("I", [BadIndex()]))
t("bad __float__", lambda: A("d", [BadFloat()]))
t("bool items", lambda: A("i", [True, False]).tolist())

calls = []


class Counted:
    def __init__(self, v):
        self.v = v

    def __index__(self):
        calls.append(self.v)
        return self.v


def count_calls(label, fn):
    calls.clear()
    try:
        result = fn()
    except Exception as exc:
        result = type(exc).__name__
    print("calls", label, len(calls), result)


count_calls("append", lambda: A("i").append(Counted(1)))
count_calls("insert", lambda: A("i", [5]).insert(0, Counted(1)))
count_calls("extend list", lambda: A("i").extend([Counted(1), Counted(2)]))
count_calls("ctor list", lambda: A("i", [Counted(1), Counted(2)]))
count_calls("ctor gen", lambda: A("i", (c for c in [Counted(1), Counted(2)])))
count_calls("fromlist", lambda: A("i").fromlist([Counted(1), Counted(2)]))
count_calls("setitem", lambda: A("i", [0]).__setitem__(0, Counted(1)))
count_calls("index start", lambda: A("i", [1, 2]).index(2, Counted(0)))
count_calls("pop", lambda: A("i", [1, 2]).pop(Counted(0)))
count_calls("getitem", lambda: A("i", [1, 2])[Counted(1)])
count_calls("repeat", lambda: A("i", [1]) * Counted(2))


# ── 3. Construction ─────────────────────────────────────────────────────────
t("ctor no args", lambda: A())
t("ctor 3 args", lambda: A("i", [], 1))
t("ctor keyword", lambda: A("i", initializer=[]))
t("ctor typecode int", lambda: A(1))
t("ctor typecode empty", lambda: A(""))
t("ctor typecode long", lambda: A("ii"))
t("ctor typecode None", lambda: A(None))
t("ctor bad typecode", lambda: A("z"))
t("ctor bad typecode + non-iterable", lambda: A("z", 5))
t("ctor bad typecode + list", lambda: A("z", [1]))
t("ctor str init", lambda: A("i", "abc"))
t("ctor unicode array init", lambda: A("i", A("u", "ab")))
t("ctor int array into u", lambda: A("u", A("i", [65])))
t("ctor non-iterable", lambda: A("i", 5))
t("ctor None init", lambda: A("i", None))
t("new not a type", lambda: A.__new__(1))
t("new not a subtype", lambda: A.__new__(int, "i"))
t("new no args", lambda: A.__new__())


class L(list):
    pass


class T(tuple):
    pass


class By(bytes):
    pass


class Ba(bytearray):
    pass


class St(str):
    pass


print("init subclasses:", A("i", L([1, 2])), A("i", T((3,))), A("b", By(b"\x01\x02")),
      A("B", Ba(b"\x05")), A("u", St("xy")))
print("init iterables:", A("i", range(3)), A("d", map(float, [1, 2])), A("i", {1: 0, 2: 0}),
      A("u", iter("ab")), A("h", A("b", [1, -1])), A("b", b""), A("u", ""))
t("init str subclass", lambda: A("i", St("1")))


class Sub(A):
    pass


class WithInit(A):
    def __init__(self, typecode, data=None, *, tag=None):
        self.tag = tag


class WithNew(A):
    def __new__(cls, *args, **kwargs):
        return super().__new__(cls, *args)


w = WithInit("i", [1], tag="t")
print("subclass __init__ kwargs:", w, w.tag, type(w).__name__)
t("subclass without __init__ kwargs", lambda: Sub("i", initializer=[1]))
print("subclass __new__:", WithNew("i", [2], extra=1))
s = Sub("i", [1, 2])
s.attr = "v"
print("subclass:", repr(s), s.__dict__, type(s[:1]).__name__, type(s + s).__name__,
      type(s * 2).__name__, type(2 * s).__name__, type(copy.copy(s)).__name__,
      type(copy.deepcopy(s)).__name__)


# ── 4. Method argument diagnostics ──────────────────────────────────────────
a = A("i", [1, 2, 3])
t("append()", lambda: a.append())
t("append(1, 2)", lambda: a.append(1, 2))
t("append(v=1)", lambda: a.append(v=1))
t("extend()", lambda: a.extend())
t("extend(bb=)", lambda: a.extend(bb=[1]))
t("extend(1, 2)", lambda: a.extend([1], [2]))
t("insert(1)", lambda: a.insert(1))
t("insert(1.5, 1)", lambda: a.insert(1.5, 1))
t("insert(huge, 1)", lambda: a.insert(1 << 70, 1))
t("insert(i=)", lambda: a.insert(i=0, v=1))
t("pop(1.5)", lambda: a.pop(1.5))
t("pop(huge)", lambda: a.pop(1 << 70))
t("pop(1, 2)", lambda: a.pop(1, 2))
t("pop(i=)", lambda: a.pop(i=1))
t("index()", lambda: a.index())
t("index(1, 1.5)", lambda: a.index(1, 1.5))
t("index(1, None)", lambda: a.index(1, None))
t("index huge bounds", lambda: a.index(3, -(1 << 70), 1 << 70))
t("index(1, 0, 1, 2)", lambda: a.index(1, 0, 1, 2))
t("index(v=)", lambda: a.index(v=1))
t("index window", lambda: (a.index(3, -1), a.index(1, -10, 1), a.index(2, 1, 2)))
t("index miss window", lambda: a.index(3, 0, 2))
t("count()", lambda: a.count())
t("remove()", lambda: a.remove())
t("remove missing", lambda: a.remove(9))
for name in ["tolist", "tobytes", "reverse", "byteswap", "buffer_info", "__copy__", "__sizeof__"]:
    t(name + "(1)", lambda: getattr(a, name)(1))
    t(name + "(x=)", lambda: getattr(a, name)(x=1))
t("__deepcopy__()", lambda: a.__deepcopy__())
t("frombytes()", lambda: a.frombytes())
t("frombytes(buffer=)", lambda: a.frombytes(buffer=b""))
t("fromlist()", lambda: a.fromlist())
t("fromunicode()", lambda: A("u").fromunicode())
t("fromfile(f)", lambda: a.fromfile(None))
t("fromfile(f=, n=)", lambda: a.fromfile(f=None, n=1))
t("tofile()", lambda: a.tofile())
t("__reduce_ex__()", lambda: a.__reduce_ex__())
t("__reduce_ex__(value=)", lambda: a.__reduce_ex__(value=3))
t("__class_getitem__()", lambda: A.__class_getitem__())
t("unbound append", lambda: A.append(1, 2))
t("unbound tolist", lambda: A.tolist(None))
t("unbound __len__", lambda: A.__len__(1))
t("unbound __getitem__", lambda: A.__getitem__(1, 0))
for name, args in [("__getitem__", ()), ("__delitem__", ()), ("__contains__", ()),
                   ("__add__", ()), ("__iadd__", ()), ("__eq__", ()), ("__len__", (1,)),
                   ("__iter__", (1,)), ("__repr__", (1,)), ("__mul__", ()), ("__rmul__", ()),
                   ("__imul__", ()), ("__setitem__", (0,)), ("__buffer__", ()),
                   ("__release_buffer__", ())]:
    t("slot " + name, lambda: getattr(a, name)(*args))
t("slot keyword", lambda: a.__getitem__(k=1))


# ── 5. Mutation semantics ───────────────────────────────────────────────────
m = A("i", [10, 20])
m.insert(-100, 1)
m.insert(100, 99)
m.insert(1, 5)
print("insert clamp:", m)
print("pop:", m.pop(), m.pop(0), m.pop(-2), m)
t("pop empty", lambda: A("i").pop())
t("pop out of range", lambda: A("i", [1]).pop(-2))


def failing_gen():
    yield 1
    yield 2
    raise ValueError("boom")


p = A("i")
t("extend gen error", lambda: p.extend(failing_gen()))
print("extend gen partial:", p)
q = A("i", [1])
t("fromlist error", lambda: q.fromlist([2, 3, "x"]))
print("fromlist atomic:", q)
r = A("i", [1, 2])
r.extend(r)
r += r
print("self extend:", r)
r[1:3] = r
print("self slice assign:", r)
t("extend other kind", lambda: A("i").extend(A("h")))
t("iadd other kind", lambda: A("i").__iadd__(A("h")))
t("iadd list", lambda: A("i").__iadd__([1]))
bs = A("h", [0x0102, -2])
bs.byteswap()
print("byteswap h:", bs.tolist())
bq = A("q", [1, -1])
bq.byteswap()
print("byteswap q:", bq.tolist())
bd = A("d", [1.5])
bd.byteswap()
print("byteswap d:", bd.tobytes())
rv = A("u", "abc")
rv.reverse()
print("reverse:", rv, A("i").reverse(), A("i", [1]))


# ── 6. Bytes, unicode and files ─────────────────────────────────────────────
fb = A("h")
fb.frombytes(memoryview(b"\x01\x00\x02\x00"))
fb.frombytes(A("B", [3, 0]))
fb.frombytes(bytearray(b"\x04\x00"))
print("frombytes sources:", fb)
t("frombytes str", lambda: fb.frombytes("ab"))
t("frombytes int", lambda: fb.frombytes(5))
t("frombytes wide array", lambda: fb.frombytes(A("i", [1])))
t("frombytes cast view", lambda: A("b").frombytes(memoryview(b"abcd").cast("I")))
t("frombytes odd length", lambda: fb.frombytes(b"\x01"))
t("frombytes self", lambda: (lambda x: (x.frombytes(x), x))(A("b", [1, 2])))
t("frombytes empty self", lambda: (lambda x: (x.frombytes(x), x))(A("b")))
print("tobytes:", A("h", [1, -1]).tobytes(), A("d", [1.5]).tobytes(), A("b").tobytes())
u = A("u", "hi")
u.fromunicode("")
u.fromunicode(St("!"))
print("unicode:", u, u.tounicode(), u.tolist(), u[0], u.itemsize == len(A("u", "x").tobytes()))
t("fromunicode bytes", lambda: A("u").fromunicode(b"x"))
t("fromunicode on i", lambda: A("i").fromunicode("x"))
t("fromunicode bytes on i", lambda: A("i").fromunicode(b"x"))
t("tounicode on i", lambda: A("i").tounicode())
nb = A("u", "\U0001F600x")
print("non-BMP:", len(nb), nb.tounicode() == "\U0001F600x", ascii(nb.tolist()))
ls = A("u", "\ud800x")
print("lone surrogate:", len(ls), ascii(ls.tounicode()), ascii(ls[0]))


class Reader:
    def __init__(self, payload):
        self.payload = payload

    def read(self, n):
        print("read", n)
        out, self.payload = self.payload[:n], self.payload[n:]
        return out


class Writer:
    def write(self, data):
        print("write", type(data).__name__, len(data))


ff = A("h")
ff.fromfile(Reader(b"\x01\x00\x02\x00\x03"), 2)
print("fromfile:", ff)
t("fromfile short", lambda: ff.fromfile(Reader(b"\x05\x00"), 2))
print("fromfile short kept:", ff)
t("fromfile partial item", lambda: A("i").fromfile(Reader(b"\x01\x02\x03\x04\x05"), 2))
t("fromfile zero", lambda: A("i").fromfile(Reader(b""), 0))
t("fromfile negative", lambda: A("i").fromfile(None, -1))
t("fromfile float", lambda: A("i").fromfile(None, 1.5))
t("fromfile no read", lambda: A("i").fromfile(None, 1))
t("fromfile bytearray", lambda: A("i").fromfile(Reader(bytearray(8)), 1))
t("tofile empty", lambda: A("i").tofile(None))
t("tofile blocks", lambda: A("d", [0.0] * 20000).tofile(Writer()))
t("tofile no write", lambda: A("i", [1]).tofile(None))
bio = io.BytesIO()
A("i", [7, 8]).tofile(bio)
bio.seek(0)
back = A("i")
back.fromfile(bio, 2)
print("BytesIO roundtrip:", back)
info = A("i", [1, 2]).buffer_info()
print("buffer_info:", type(info).__name__, len(info), type(info[0]).__name__, info[1], A("i").buffer_info()[1])


# ── 7. Sequence protocol and slicing ────────────────────────────────────────
sq = A("i", [5, 6, 7])
t("getitem str", lambda: sq["x"])
t("getitem float", lambda: sq[1.5])
t("getitem huge", lambda: sq[1 << 70])
t("getitem oob", lambda: sq[3])
t("setitem oob", lambda: sq.__setitem__(-4, 1))
t("setitem str", lambda: sq.__setitem__("x", 1))
t("delitem oob", lambda: sq.__delitem__(3))
t("delitem str", lambda: sq.__delitem__("x"))
t("setslice list", lambda: sq.__setitem__(slice(0, 1), [1]))
t("setslice other kind", lambda: sq.__setitem__(slice(0, 1), A("d", [1])))
t("setslice extended size", lambda: sq.__setitem__(slice(0, 3, 2), A("i", [1])))
t("slice step zero", lambda: sq[::0])
t("index protocol", lambda: (sq[Counted(1)], sq[Counted(1):], sq[True], sq[-1]))
print("iteration:", list(sq), list(reversed(sq)), sorted(A("d", [3, 1, 2])), sum(sq), max(sq))
print("contains:", 6 in sq, 6.0 in sq, None in sq, "a" in A("u", "xa"),
      float("nan") in A("d", [float("nan")]))
print("bool/len:", bool(A("i")), bool(A("i", [0])), len(sq))

base = list(range(10))
for start in (None, -12, -3, 0, 2, 9, 12):
    for stop in (None, -12, -2, 0, 3, 10, 15):
        for step in (None, -3, -1, 1, 2, 4):
            sl = slice(start, stop, step)
            picked = list(range(len(base))[sl])
            assert A("i", base)[sl].tolist() == [base[i] for i in picked], sl
            deleted = A("i", base)
            del deleted[sl]
            doomed = set(picked)
            assert deleted.tolist() == [x for i, x in enumerate(base) if i not in doomed], ("del", sl)
            assigned = A("i", base)
            assigned[sl] = A("i", range(100, 100 + len(picked)))
            expected = base[:]
            for i, v in zip(picked, range(100, 100 + len(picked))):
                expected[i] = v
            assert assigned.tolist() == expected, ("set", sl)
            if step in (None, 1):
                lo, hi, _ = sl.indices(len(base))
                hi = max(hi, lo)
                grown = A("i", base)
                grown[sl] = A("i", [7, 7, 7])
                assert grown.tolist() == base[:lo] + [7, 7, 7] + base[hi:], ("grow", sl)
print("slicing matches list semantics")


# ── 8. Live iterator ────────────────────────────────────────────────────────
live = A("i", [1, 2, 3])
seen = []
for x in live:
    seen.append(x)
    if x == 1:
        live.append(4)
print("live iteration:", seen)
it = iter(A("i", [1, 2]))
print("iter self:", iter(it) is it, next(it), it.__reduce__()[1:], list(it), list(it))
print("exhausted reduce:", it.__reduce__()[1])
grow = A("i", [1])
it = iter(grow)
next(it)
grow.append(2)
print("resume after append:", list(it))
it = iter(A("i", [1, 2, 3]))
it.__setstate__(-5)
print("setstate negative:", list(it))
it = iter(A("i", [1, 2, 3]))
it.__setstate__(2)
print("setstate 2:", list(it))
it = iter(A("i", [1, 2, 3]))
it.__setstate__(100)
print("setstate past end:", list(it))
t("setstate str", lambda: iter(A("i")).__setstate__("x"))
t("setstate float", lambda: iter(A("i")).__setstate__(1.5))
t("setstate no args", lambda: iter(A("i")).__setstate__())
t("next args", lambda: iter(A("i")).__next__(1))
t("reduce args", lambda: iter(A("i")).__reduce__(1))
t("create iterator", lambda: it_type())
t("subclass iterator", lambda: type("X", (it_type,), {}))
mid = iter(A("d", [1.0, 2.0, 3.0]))
next(mid)
print("iterator pickle:", list(pickle.loads(pickle.dumps(mid))), list(mid))


# ── 9. Operators and comparison ─────────────────────────────────────────────
t("add list", lambda: A("i") + [1])
t("add other kind", lambda: A("i") + A("d"))
t("mul float", lambda: A("i", [1]) * 1.5)
t("rmul float", lambda: 1.5 * A("i", [1]))
t("mul huge", lambda: A("i", [1]) * (1 << 70))
print("repeat:", A("i", [1, 2]) * 2, 0 * A("i", [1]), A("i", [1]) * -1, A("i") * 5, A("i", [3]) * True)
imul = A("i", [1, 2])
imul *= 1
imul *= 3
print("imul:", imul)
imul *= 0
print("imul 0:", imul)


class Reflected:
    def __radd__(self, other):
        return "radd"

    def __rmul__(self, other):
        return "rmul"


print("reflected:", A("i") + Reflected(), A("i") * Reflected())
acc = A("i")
acc += Reflected()
print("iadd reflected:", acc)
acc = A("i", [1])
acc *= Reflected()
print("imul reflected:", acc)
t("iadd op list", lambda: exec("x = A('i'); x += [1]", {"A": A}))
t("imul op float", lambda: exec("x = A('i'); x *= 1.5", {"A": A}))
t("lt list", lambda: A("i") < [1])
t("u lt i", lambda: A("u", "a") < A("i", [97]))
print("eq list:", A("i", [1]) == [1], A("i", [1]) != [1], A("i").__eq__([1]))
values = [A("i", [1, 2]), A("i", [1, 3]), A("i", [1]), A("d", [1.0, 2.0]), A("f", [1.5]),
          A("B", [1, 2]), A("i"), A("u", "ab"), A("u", "b"), A("Q", [2 ** 64 - 1]), A("q", [-1])]
for x in values:
    print("cmp", [(x < y, x <= y, x == y, x != y, x > y, x >= y)
                  for y in values if (x.typecode == "u") == (y.typecode == "u")])
nan = A("d", [float("nan")])
print("nan:", nan == nan, nan != nan, nan < nan, nan <= nan, nan > nan, nan >= nan)


class AlwaysEqual:
    def __eq__(self, other):
        return True


rem = A("i", [7, 8])
rem.remove(AlwaysEqual())
print("user __eq__:", A("i", [1, 2]).count(AlwaysEqual()), A("i", [1, 2]).index(AlwaysEqual()),
      AlwaysEqual() in A("i", [3]), rem)
t("hash", lambda: hash(A("i")))
print("repr:", repr(A("d", [0.1, -0.0])), str(A("f", [0.1])), repr(A("u", "a'b")), format(A("i", [1])))
print("repr subclass:", repr(Sub("i")), repr(Sub("u", "ab")), repr(Sub("d", [1.5])))


# ── 10. Attributes ──────────────────────────────────────────────────────────
at = A("i")
t("set attr", lambda: setattr(at, "x", 1))
t("set typecode", lambda: setattr(at, "typecode", "d"))
t("del typecode", lambda: delattr(at, "typecode"))
t("set itemsize", lambda: setattr(at, "itemsize", 1))
t("missing attr", lambda: at.nope)
t("unbound typecode", lambda: A.typecode.__get__(1))
print("typecodes/itemsizes:", [(tc, A(tc).typecode, A(tc).itemsize) for tc in array.typecodes])


# ── 11. Buffer protocol ─────────────────────────────────────────────────────
ex = A("i", [1, 2, 3])
with memoryview(ex) as mv:
    print("memoryview:", mv.format, mv.itemsize, mv.nbytes, mv.shape, mv.readonly,
          mv.obj is ex, mv.tolist())
    for label, fn in [
        ("append", lambda: ex.append(1)),
        ("extend", lambda: ex.extend([1])),
        ("extend empty", lambda: ex.extend([])),
        ("insert", lambda: ex.insert(0, 1)),
        ("pop", lambda: ex.pop()),
        ("remove", lambda: ex.remove(1)),
        ("frombytes", lambda: ex.frombytes(b"\0" * 4)),
        ("frombytes empty", lambda: ex.frombytes(b"")),
        ("fromlist", lambda: ex.fromlist([1])),
        ("fromlist empty", lambda: ex.fromlist([])),
        ("delitem", lambda: ex.__delitem__(0)),
        ("del empty slice", lambda: ex.__delitem__(slice(0, 0))),
        ("iadd", lambda: ex.__iadd__(A("i", [1]))),
        ("iadd empty", lambda: ex.__iadd__(A("i"))),
        ("imul 2", lambda: ex.__imul__(2)),
        ("imul 1", lambda: ex.__imul__(1)),
        ("imul 0", lambda: ex.__imul__(0)),
        ("setitem", lambda: ex.__setitem__(0, 9)),
        ("set slice same size", lambda: ex.__setitem__(slice(0, 1), A("i", [8]))),
        ("set slice grow", lambda: ex.__setitem__(slice(0, 1), A("i", [8, 9]))),
        ("reverse", lambda: ex.reverse()),
        ("byteswap twice", lambda: (ex.byteswap(), ex.byteswap())),
    ]:
        t("exported " + label, lambda: (fn(), ex.tolist())[1])
    mv[0] = 42
    print("write-through:", ex, mv.tolist())
ex.append(4)
print("after release:", ex)
held = memoryview(ex)
t("release_buffer", lambda: (ex.__release_buffer__(held), ex.append(5), ex)[2])
t("release_buffer again", lambda: ex.__release_buffer__(held))
t("release_buffer int", lambda: ex.__release_buffer__(1))
t("release_buffer foreign", lambda: ex.__release_buffer__(memoryview(b"x")))
t("buffer flags str", lambda: ex.__buffer__("x"))
t("buffer flags huge", lambda: ex.__buffer__(1 << 40))
print("bytes():", bytes(A("h", [1, 256])), bytearray(A("b", [-1])), bytes(A("u", "a")))
print("cast:", memoryview(A("i", [1, 2])).cast("B").tolist())
print("unicode buffer:", memoryview(A("u", "ab")).format, memoryview(A("u", "ab")).nbytes)


# ── 12. Pickling, copying, reconstruction ───────────────────────────────────
for tc in array.typecodes:
    src = A(tc, "ab" if tc == "u" else [1, 2])
    reduced = src.__reduce_ex__(4)
    print("pickle", tc,
          all(pickle.loads(pickle.dumps(src, proto)) == src for proto in range(pickle.HIGHEST_PROTOCOL + 1)),
          copy.copy(src) == src, copy.deepcopy(src) == src,
          reduced[0] is array._array_reconstructor, reduced[1][2], reduced[2])
print("reduce_ex 2:", A("i", [1, 2]).__reduce_ex__(2), A("u", "ab").__reduce_ex__(0))
print("reduce_ex bool:", A("i", [1]).__reduce_ex__(True))
t("reduce_ex str", lambda: A("i").__reduce_ex__("x"))
t("reduce_ex huge", lambda: A("i").__reduce_ex__(1 << 70))
ps = Sub("i", [1])
ps.attr = "v"
print("reduce_ex subclass:", ps.__reduce_ex__(3)[1][0].__name__, ps.__reduce_ex__(3)[2],
      Sub("i").__reduce_ex__(3)[2], ps.__reduce_ex__(1)[2])
rt = pickle.loads(pickle.dumps(ps))
print("pickle subclass:", type(rt).__name__, rt, rt.attr)
R = array._array_reconstructor
t("rec no args", lambda: R())
t("rec keyword", lambda: R(A, "i", 8, b"", x=1))
t("rec not a type", lambda: R(1, "i", 8, b""))
t("rec not a subtype", lambda: R(int, "i", 8, b""))
t("rec typecode int", lambda: R(A, 1, 8, b""))
t("rec typecode long", lambda: R(A, "ii", 8, b""))
t("rec bad typecode", lambda: R(A, "z", 8, b""))
t("rec mformat str", lambda: R(A, "i", "x", b""))
t("rec mformat 22", lambda: R(A, "i", 22, b""))
t("rec mformat -1", lambda: R(A, "i", -1, b""))
t("rec mformat huge", lambda: R(A, "i", 1 << 40, b""))
t("rec items list", lambda: R(A, "i", 8, [1]))
t("rec native", lambda: R(A, "i", A("i").__reduce_ex__(3)[1][2], A("i", [5]).tobytes()))
t("rec slow bad length", lambda: R(A, "i", 9, b"\x01"))
t("rec s32be", lambda: R(A, "i", 9, b"\x00\x00\x00\x01\xff\xff\xff\xfe"))
t("rec u64le", lambda: R(A, "i", 10, b"\x01" + b"\x00" * 7))
t("rec s64be", lambda: R(A, "i", 13, b"\x00" * 7 + b"\x05"))
t("rec u16be", lambda: R(A, "b", 3, b"\x01\x02"))
t("rec s16le", lambda: R(A, "H", 4, b"\xfe\xff"))
t("rec u8", lambda: R(A, "i", 0, b"\x01\x02"))
t("rec s8 into d", lambda: R(A, "d", 1, b"\xff"))
t("rec float be", lambda: R(A, "f", 15, b"\x3f\xc0\x00\x00"))
t("rec float le into d", lambda: R(A, "d", 14, b"\x00\x00\xc0\x3f"))
t("rec double be into f", lambda: R(A, "f", 17, b"\x3f\xf8" + b"\x00" * 6))
t("rec utf16be", lambda: R(A, "u", 19, b"\x00a\x00b"))
t("rec utf16le", lambda: R(A, "u", 18, b"a\x00b\x00"))
t("rec utf16be odd", lambda: R(A, "u", 19, b"\x00a"))
t("rec utf32le", lambda: R(A, "u", 20, b"a\x00\x00\x00b\x00\x00\x00"))
t("rec utf32be", lambda: R(A, "u", 21, b"\x00\x00\x00a\x00\x00\x00b"))
t("rec utf16 into i", lambda: R(A, "i", 19, b"\x00a\x00b"))
t("rec utf16 bad surrogate", lambda: R(A, "u", 19, b"\xd8\x00\x00a"))
t("rec subclass", lambda: type(R(Sub, "i", 8, b"\x01\x00\x00\x00")).__name__)


# ── 13. `__sizeof__` over-allocation model ──────────────────────────────────
print("sizeof empty:", [A(tc).__sizeof__() for tc in "bhid"])
print("sizeof exact:", [A(tc, [1, 2, 3]).__sizeof__() for tc in "bhid"])
grown = A("h")
sizes = []
for i in range(30):
    grown.append(i)
    sizes.append(grown.__sizeof__())
for _ in range(30):
    grown.pop()
sizes.append(grown.__sizeof__())
print("sizeof growth:", sizes)
model = A("h", range(10))
steps = []
model *= 3
steps.append(model.__sizeof__())
del model[5:25]
steps.append(model.__sizeof__())
model.extend(range(40))
steps.append(model.__sizeof__())
model.extend(A("h", range(40)))
steps.append(model.__sizeof__())
model.frombytes(b"\0" * 200)
steps.append(model.__sizeof__())
model.fromlist(list(range(5)))
steps.append(model.__sizeof__())
model[2:3] = A("h", range(50))
steps.append(model.__sizeof__())
del model[::2]
steps.append(model.__sizeof__())
model[:] = A("h")
steps.append(model.__sizeof__())
print("sizeof steps:", steps)
print("sizeof inits:", A("h", (x for x in range(10))).__sizeof__(), A("h", A("h", range(10))).__sizeof__(),
      A("h", A("b", range(10))).__sizeof__(), A("h", range(10))[1:4].__sizeof__(),
      (A("h", range(10)) * 3).__sizeof__(), A("u", "hello").__sizeof__())


class Slotted(A):
    __slots__ = ("x", "y")


print("sizeof subclasses:", Sub("i", [1, 2]).__sizeof__(), Slotted("i", [1, 2]).__sizeof__())
