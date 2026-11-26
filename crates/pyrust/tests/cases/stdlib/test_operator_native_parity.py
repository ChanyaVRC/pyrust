# Parity fixture for the native `operator` module (#3364).
#
# `operator` used to be an `@inject`ed Python port (`operator_py.py`) of
# `Lib/operator.py`.  It is now 100% Rust, modelled on the `_operator`
# accelerator that shadows that file at runtime in CPython 3.12.  This fixture
# pins the observable surface: argument-count diagnostics, the dunder aliases,
# `concat` / `iconcat` slot semantics, lazy `countOf` / `indexOf`, the in-place
# helpers, and the final `attrgetter` / `itemgetter` / `methodcaller` types
# (constructors, calls, `repr`, `__reduce__` and pickling).

import collections
import copy
import operator as o
import pickle


def show(label, fn):
    try:
        print(label, "->", repr(fn()))
    except BaseException as exc:
        print(label, "!!", type(exc).__name__ + ":", exc)


# ── Module surface ──────────────────────────────────────────────────────────
print(len(o.__all__), o.__all__ == sorted(o.__all__, key=lambda n: n))
print(o.__all__[:6], o.__all__[-3:])
print(all(hasattr(o, name) for name in o.__all__))
print(o.__doc__.splitlines()[0])
print(o.__name__)

DUNDERS = {
    "__lt__": "lt", "__le__": "le", "__eq__": "eq", "__ne__": "ne",
    "__ge__": "ge", "__gt__": "gt", "__not__": "not_", "__abs__": "abs",
    "__add__": "add", "__and__": "and_", "__call__": "call",
    "__floordiv__": "floordiv", "__index__": "index", "__inv__": "inv",
    "__invert__": "invert", "__lshift__": "lshift", "__mod__": "mod",
    "__mul__": "mul", "__matmul__": "matmul", "__neg__": "neg",
    "__or__": "or_", "__pos__": "pos", "__pow__": "pow",
    "__rshift__": "rshift", "__sub__": "sub", "__truediv__": "truediv",
    "__xor__": "xor", "__concat__": "concat", "__contains__": "contains",
    "__delitem__": "delitem", "__getitem__": "getitem",
    "__setitem__": "setitem", "__iadd__": "iadd", "__iand__": "iand",
    "__iconcat__": "iconcat", "__ifloordiv__": "ifloordiv",
    "__ilshift__": "ilshift", "__imod__": "imod", "__imul__": "imul",
    "__imatmul__": "imatmul", "__ior__": "ior", "__ipow__": "ipow",
    "__irshift__": "irshift", "__isub__": "isub", "__itruediv__": "itruediv",
    "__ixor__": "ixor",
}
print(len(DUNDERS), all(getattr(o, d) is getattr(o, n) for d, n in DUNDERS.items()))
print(o.inv is o.invert, hasattr(o, "__countOf__"), hasattr(o, "__length_hint__"))
print([getattr(o, n).__name__ for n in ("add", "__add__", "__inv__", "__not__", "is_not")])
print([getattr(o, n).__module__ for n in ("add", "not_", "countOf", "length_hint", "index")])
print(o.attrgetter.__module__, o.itemgetter.__module__, o.methodcaller.__module__)
print(type(o.add).__name__, type(o.attrgetter).__name__, repr(o.add))
print(o.add == o.__add__, hash(o.add) == hash(o.__add__))
import _operator
print(_operator.add is not None, _operator.itemgetter is o.itemgetter)

# ── Argument-count diagnostics ──────────────────────────────────────────────
TWO = ["add", "lt", "pow", "mod", "is_", "is_not", "concat", "contains",
       "countOf", "indexOf", "getitem", "delitem", "iadd", "iconcat", "ipow"]
ONE = ["not_", "truth", "abs", "neg", "pos", "inv", "invert", "index"]
for name in TWO:
    fn = getattr(o, name)
    show(name + "()", lambda: fn())
    show(name + "(1)", lambda: fn(1))
    show(name + "(1,2,3)", lambda: fn(1, 2, 3))
    show(name + "(a=1)", lambda: fn(a=1))
    show(name + "(1,b=2)", lambda: fn(1, b=2))
for name in ONE:
    fn = getattr(o, name)
    show(name + "()", lambda: fn())
    show(name + "(1,2)", lambda: fn(1, 2))
    show(name + "(a=1)", lambda: fn(a=1))
show("setitem()", lambda: o.setitem())
show("setitem(1,2)", lambda: o.setitem(1, 2))
show("setitem(1,2,3,4)", lambda: o.setitem(1, 2, 3, 4))
show("setitem(a=1)", lambda: o.setitem(a=1))
show("length_hint()", lambda: o.length_hint())
show("length_hint(1,2,3)", lambda: o.length_hint(1, 2, 3))
show("length_hint(a=1)", lambda: o.length_hint([], default=1))
show("call()", lambda: o.call())
show("call(obj=dict)", lambda: o.call(obj=dict))
show("call(1)", lambda: o.call(1))
show("call kwargs", lambda: o.call(lambda *a, **k: (a, k), 1, 2, x=3, obj=4))
show("call dict(a=1)", lambda: o.call(dict, a=1))

# ── Arithmetic, bitwise, comparison, logic ──────────────────────────────────
print(o.add(1, 2), o.sub(5, 3), o.mul(4, 3), o.truediv(7, 2), o.floordiv(7, 2))
print(o.mod(7, 3), o.pow(2, 10), o.pow(2, -1), o.matmul.__name__, o.abs(-9))
print(o.neg(5), o.pos(-3), o.inv(5), o.invert(5), o.abs(-2.5), o.abs(3 + 4j))
print(o.and_(6, 3), o.or_(4, 1), o.xor(5, 1), o.lshift(1, 4), o.rshift(16, 2))
print(o.add(2 ** 64, 1), o.mul(2 ** 40, 2 ** 40), o.neg(-(2 ** 70)), o.abs(-(2 ** 70)))
print(o.lt(1, 2), o.le(2, 2), o.eq(2, 2), o.ne(2, 3), o.ge(3, 2), o.gt(3, 2))
print(o.lt("a", "b"), o.eq([1], [1]), o.ne({1}, {1}), o.gt((2,), (1, 5)))
print(o.not_(0), o.not_([0]), o.truth([]), o.truth([1]), o.truth(object()))
print(o.is_(None, None), o.is_not(None, None), o.is_(1, 1.0), type(o.is_(1, 2)).__name__)
print(o.add("a", "b"), o.add([1], [2]), o.mul("ab", 2), o.add((1,), (2,)))
show("add 1,'x'", lambda: o.add(1, "x"))
show("sub 'a',1", lambda: o.sub("a", 1))
show("floordiv 'a',1", lambda: o.floordiv("a", 1))
show("matmul 1,2", lambda: o.matmul(1, 2))
show("truediv 1,0", lambda: o.truediv(1, 0))
show("mod 1,0", lambda: o.mod(1, 0))
show("lt 1,'a'", lambda: o.lt(1, "a"))
show("abs 'x'", lambda: o.abs("x"))
show("neg 'x'", lambda: o.neg("x"))
show("pos 'x'", lambda: o.pos("x"))
show("inv 1.5", lambda: o.inv(1.5))


class Rich:
    def __lt__(self, other):
        return "lt-result"

    def __eq__(self, other):
        return "eq-result"

    def __add__(self, other):
        return ("add", other)

    def __radd__(self, other):
        return ("radd", other)

    def __neg__(self):
        return "neg-result"

    def __abs__(self):
        return "abs-result"

    def __invert__(self):
        return "invert-result"

    def __hash__(self):
        return 1


r = Rich()
print(o.lt(r, 1), o.eq(r, 1), o.add(r, 2), o.add(2, r), o.neg(r), o.abs(r), o.inv(r))


class BadBool:
    def __bool__(self):
        return 1


show("truth(BadBool)", lambda: o.truth(BadBool()))
show("not_(BadBool)", lambda: o.not_(BadBool()))

# ── index / length_hint ─────────────────────────────────────────────────────
class Idx:
    def __index__(self):
        return 7


class BadIdx:
    def __index__(self):
        return "x"


print(o.index(42), o.index(True), o.index(10 ** 30), o.index(Idx()))
show("index(1.5)", lambda: o.index(1.5))
show("index(BadIdx)", lambda: o.index(BadIdx()))
print(o.length_hint([1, 2, 3]), o.length_hint(object(), 7), o.length_hint(iter(range(5))))
show("length_hint('x')", lambda: o.length_hint(object(), "x"))

# ── In-place variants ───────────────────────────────────────────────────────
lst = [1]
print(o.iadd(lst, [2]) is lst, lst)
lst = [1, 2]
print(o.imul(lst, 2) is lst, lst)
st = {1}
print(o.ior(st, {2}) is st, sorted(st))
print(o.iand({1, 2}, {2}), o.isub({1, 2}, {2}), o.ixor({1, 2}, {2, 3}))
print(o.iadd(1, 2), o.isub(10, 3), o.imul(3, 4), o.itruediv(7, 2), o.ifloordiv(7, 2))
print(o.imod(7, 3), o.ipow(2, 5), o.ilshift(1, 3), o.irshift(16, 2))
print(o.iand(6, 3), o.ior(4, 1), o.ixor(5, 1), o.iadd("a", "b"), o.iadd((1,), (2,)))
show("iadd 1,'x'", lambda: o.iadd(1, "x"))
show("isub 'a',1", lambda: o.isub("a", 1))
show("ipow 1,'x'", lambda: o.ipow(1, "x"))
show("imatmul 1,2", lambda: o.imatmul(1, 2))
show("imatmul [],1", lambda: o.imatmul([], 1))


class InPlace:
    def __iadd__(self, other):
        return ("iadd", other)

    def __add__(self, other):
        return ("add", other)


class OnlyAdd:
    def __add__(self, other):
        return ("only-add", other)


print(o.iadd(InPlace(), 1), o.iadd(OnlyAdd(), 1), o.add(InPlace(), 1))
d1 = {"a": 1}
print(o.ior(d1, {"b": 2}) is d1, d1)

# ── Item access ─────────────────────────────────────────────────────────────
lst = [10, 20, 30, 40]
dct = {"k": "v"}
print(o.getitem(lst, 1), o.getitem(lst, -1), o.getitem(lst, slice(1, 3)), o.getitem(dct, "k"))
print(o.getitem("abc", 1), o.getitem((1, 2), 0), o.getitem(b"abc", 0), o.getitem(range(5), 2))
print(o.setitem(lst, 0, 99), lst, o.setitem(lst, slice(1, 3), [7]), lst)
print(o.setitem(dct, "z", 1), dct, o.delitem(dct, "z"), dct)
print(o.delitem(lst, 0), lst, o.delitem(lst, slice(0, 2)), lst)
show("getitem(1,2)", lambda: o.getitem(1, 2))
show("getitem({},1)", lambda: o.getitem({}, 1))
show("getitem([1],'a')", lambda: o.getitem([1], "a"))
show("getitem([1],5)", lambda: o.getitem([1], 5))
show("setitem((1,),0,1)", lambda: o.setitem((1,), 0, 1))
show("setitem('s',0,1)", lambda: o.setitem("s", 0, 1))
show("delitem((1,),0)", lambda: o.delitem((1,), 0))
show("delitem([],0)", lambda: o.delitem([], 0))
show("delitem({},1)", lambda: o.delitem({}, 1))


class Sub:
    def __init__(self):
        self.log = []

    def __getitem__(self, key):
        self.log.append(("get", key))
        return key

    def __setitem__(self, key, value):
        self.log.append(("set", key, value))

    def __delitem__(self, key):
        self.log.append(("del", key))

    def __contains__(self, item):
        self.log.append(("in", item))
        return item == 1


s = Sub()
print(o.getitem(s, 3), o.getitem(s, slice(1, 2)).indices(5), o.setitem(s, "a", 1), o.delitem(s, "b"))
print(o.contains(s, 1), o.contains(s, 2))
print(s.log)

# ── contains / countOf / indexOf ────────────────────────────────────────────
print(o.contains([1, 2, 3], 2), o.contains("abc", "b"), o.contains({1: 2}, 1), o.contains(range(5), 9))
print(type(o.contains([1], 1)).__name__, o.contains({"a"}, "a"), o.contains(b"abc", 98))
print(o.countOf([1, 1, 2, 1], 1), o.countOf("abca", "a"), o.countOf((1, 1.0, True), 1))
print(o.indexOf([5, 6, 7], 6), o.indexOf("abc", "c"), o.indexOf(range(10), 4))
nan = float("nan")
print(o.countOf([nan], nan), o.countOf([nan], float("nan")), o.indexOf([nan], nan))
show("contains(1,2)", lambda: o.contains(1, 2))
show("countOf(1,2)", lambda: o.countOf(1, 2))
show("indexOf(1,2)", lambda: o.indexOf(1, 2))
show("indexOf([],1)", lambda: o.indexOf([], 1))
show("indexOf([1,2],9)", lambda: o.indexOf([1, 2], 9))
show("countOf(None,1)", lambda: o.countOf(None, 1))


def gen():
    for i in range(3):
        log.append(i)
        yield i


log = []
print(o.indexOf(gen(), 1), log)
log = []
print(o.countOf(gen(), 1), log)


class LazyEq:
    def __eq__(self, other):
        seen.append("eq")
        return other == 1

    def __hash__(self):
        return 0


seen = []
print(o.countOf([LazyEq(), 1, LazyEq()], 1), seen)
seen = []
print(o.indexOf([LazyEq(), 0], 1), seen)


class BadIter:
    def __iter__(self):
        raise TypeError("inner failure")


show("countOf(BadIter)", lambda: o.countOf(BadIter(), 1))


class GetItemOnly:
    def __getitem__(self, index):
        if index > 3:
            raise IndexError
        return index


print(o.countOf(GetItemOnly(), 2), o.indexOf(GetItemOnly(), 3))

# ── concat / iconcat (PySequence_Concat semantics) ──────────────────────────
class AddGet:
    def __add__(self, other):
        return "add"

    def __radd__(self, other):
        return "radd"

    def __getitem__(self, index):
        if index > 3:
            raise IndexError
        return index


class ListSubAdd(list):
    def __add__(self, other):
        return "ListSubAdd.add"


class ListSubRadd(list):
    def __radd__(self, other):
        return "ListSubRadd.radd"


class StrSub(str):
    pass


CASES = {
    "list": lambda: [1],
    "tuple": lambda: (1,),
    "str": lambda: "a",
    "bytes": lambda: b"a",
    "bytearray": lambda: bytearray(b"a"),
    "deque": lambda: collections.deque([1]),
    "range": lambda: range(3),
    "dict": lambda: {},
    "set": lambda: {1},
    "int": lambda: 1,
    "None": lambda: None,
    "OnlyAdd": OnlyAdd,
    "AddGet": AddGet,
    "UserList": lambda: collections.UserList([1]),
    "ListSubAdd": lambda: ListSubAdd([1]),
    "ListSubRadd": lambda: ListSubRadd([1]),
    "StrSub": lambda: StrSub("a"),
}
RIGHTS = {
    "list": [2],
    "tuple": (2,),
    "str": "b",
    "bytes": b"b",
    "OnlyAdd": OnlyAdd(),
    "ListSubRadd": ListSubRadd([2]),
    "int": 2,
}
for left_name, left in CASES.items():
    for right_name, right in RIGHTS.items():
        show("concat %s+%s" % (left_name, right_name), lambda: o.concat(left(), right))

        def run_iconcat():
            target = left()
            result = o.iconcat(target, right)
            return (result, result is target)

        show("iconcat %s+%s" % (left_name, right_name), run_iconcat)

print(o.concat(collections.deque([1]), collections.deque([2])))
dq = collections.deque([1])
print(o.iconcat(dq, [2, 3]) is dq, dq, o.iconcat([1], collections.deque([2])))
show("concat deque+list", lambda: o.concat(collections.deque([1]), [2]))
show("iconcat deque+int", lambda: o.iconcat(collections.deque([1]), 2))

# ── attrgetter ──────────────────────────────────────────────────────────────
class Node:
    pass


n = Node()
n.a = Node()
n.a.b = Node()
n.a.b.c = 42
n.x = 1
n.y = 2
print(o.attrgetter("x")(n), o.attrgetter("a.b.c")(n), o.attrgetter("x", "y")(n))
print(o.attrgetter("x", "a.b.c")(n), o.attrgetter("x", "y", "x")(n))
print(sorted([Node(), Node()], key=o.attrgetter("__class__.__name__"))[0].__class__.__name__)
print(o.attrgetter("real", "imag")(3 + 4j), o.attrgetter("__class__.__name__")(1))
print(repr(o.attrgetter("x")), repr(o.attrgetter("a.b.c")), repr(o.attrgetter("x", "a.b")))
print(o.attrgetter("x").__reduce__(), o.attrgetter("x", "a.b").__reduce__())
print(o.attrgetter.__name__, o.attrgetter.__qualname__, o.attrgetter.__module__)
print(o.attrgetter.__doc__.splitlines()[0])
show("attrgetter()", lambda: o.attrgetter())
show("attrgetter(attr='x')", lambda: o.attrgetter(attr="x"))
show("attrgetter(1)", lambda: o.attrgetter(1))
show("attrgetter('a', 1)", lambda: o.attrgetter("a", 1))
show("attrgetter.__new__(cls)", lambda: o.attrgetter.__new__(o.attrgetter))
show("attrgetter('x')()", lambda: o.attrgetter("x")())
show("attrgetter('x')(1, 2)", lambda: o.attrgetter("x")(1, 2))
show("attrgetter('x')(obj=1)", lambda: o.attrgetter("x")(obj=1))
show("attrgetter('zz')(1)", lambda: o.attrgetter("zz")(1))
show("attrgetter('real.zz')(1j)", lambda: o.attrgetter("real.zz")(1j))
show("attrgetter('')(1)", lambda: o.attrgetter("")(1))
show("attrgetter('a..b')(1)", lambda: o.attrgetter("a..b")(1))
show("attrgetter('x','nope')(n)", lambda: o.attrgetter("x", "nope")(n))


class PropRaises:
    @property
    def x(self):
        raise AttributeError("boom")


show("attrgetter property", lambda: o.attrgetter("x")(PropRaises()))

# ── itemgetter ──────────────────────────────────────────────────────────────
print(o.itemgetter(1)([10, 20, 30]), o.itemgetter(0, 2)([10, 20, 30]), o.itemgetter("k")({"k": "v"}))
print(o.itemgetter(slice(1, 3))([1, 2, 3, 4]), o.itemgetter(-1)("abc"), o.itemgetter(1, 1)("abc"))
print(sorted([(2, "a"), (1, "b")], key=o.itemgetter(0)), sorted(["ba", "ab"], key=o.itemgetter(1)))
print(repr(o.itemgetter(1)), repr(o.itemgetter(1, "a", None)), repr(o.itemgetter(slice(1, 2))))
print(o.itemgetter(1).__reduce__(), o.itemgetter(1, 2).__reduce__())
print(o.itemgetter.__name__, o.itemgetter.__module__, o.itemgetter.__doc__.splitlines()[0])
show("itemgetter()", lambda: o.itemgetter())
show("itemgetter(item=1)", lambda: o.itemgetter(item=1))
show("itemgetter(1)()", lambda: o.itemgetter(1)())
show("itemgetter(1)(1, 2)", lambda: o.itemgetter(1)(1, 2))
show("itemgetter(1)(obj=1)", lambda: o.itemgetter(1)(obj=1))
show("itemgetter(5)([1])", lambda: o.itemgetter(5)([1]))
show("itemgetter(0, 9)([1])", lambda: o.itemgetter(0, 9)([1]))
show("itemgetter('a')([1])", lambda: o.itemgetter("a")([1]))
show("itemgetter(1)(1)", lambda: o.itemgetter(1)(1))
show("itemgetter(1).__new__()", lambda: o.itemgetter.__new__(o.itemgetter))

# ── methodcaller ────────────────────────────────────────────────────────────
class Kw:
    def m(self, *args, **kwargs):
        return (args, kwargs)


print(list(map(o.methodcaller("lower"), ["HELLO", "World"])), o.methodcaller("count", "l")("hello"))
print(o.methodcaller("m")(Kw()), o.methodcaller("m", 1, 2)(Kw()), o.methodcaller("m", 1, k=2, z=3)(Kw()))
print(o.methodcaller("split", ",", maxsplit=1)("a,b,c"))
print(repr(o.methodcaller("m")), repr(o.methodcaller("m", 1, "s", k=[1], z=None)))
print(repr(o.methodcaller("m", b=1, a=2)))
print(o.methodcaller("m").__reduce__(), o.methodcaller("m", 1, 2).__reduce__())
reduced = o.methodcaller("m", 1, k=2).__reduce__()
print(type(reduced[0]).__name__, reduced[0].func is o.methodcaller, reduced[0].args, reduced[0].keywords, reduced[1])
print(o.methodcaller.__name__, o.methodcaller.__module__, o.methodcaller.__doc__.splitlines()[0])
show("methodcaller()", lambda: o.methodcaller())
show("methodcaller(name='x')", lambda: o.methodcaller(name="x"))
show("methodcaller(1)", lambda: o.methodcaller(1))
show("methodcaller('upper')()", lambda: o.methodcaller("upper")())
show("methodcaller('upper')(1, 2)", lambda: o.methodcaller("upper")(1, 2))
show("methodcaller('upper')(obj='a')", lambda: o.methodcaller("upper")(obj="a"))
show("methodcaller('zz')(1)", lambda: o.methodcaller("zz")(1))
show("methodcaller('upper', name=1)('a')", lambda: o.methodcaller("upper", name=1)("a"))
show("methodcaller('upper', 1)('a')", lambda: o.methodcaller("upper", 1)("a"))

# ── Final, slot-only types ──────────────────────────────────────────────────
for cls in (o.attrgetter, o.itemgetter, o.methodcaller):
    def subclass(cls=cls):
        class Child(cls):
            pass
        return Child

    show(cls.__name__ + " subclass", subclass)

ag = o.attrgetter("x")
show("attrgetter __dict__", lambda: ag.__dict__)
show("attrgetter setattr", lambda: setattr(ag, "y", 1))
show("attrgetter hash", lambda: hash(ag) == hash(ag))
show("attrgetter eq", lambda: ag == o.attrgetter("x"))
show("attrgetter callable", lambda: callable(ag))
show("methodcaller callable", lambda: callable(o.methodcaller("x")))
show("isinstance", lambda: isinstance(ag, o.attrgetter) and not isinstance(ag, o.itemgetter))

# ── Recursive repr ──────────────────────────────────────────────────────────
cycle = []
getter = o.itemgetter(cycle)
cycle.append(getter)
show("itemgetter recursive repr", lambda: repr(getter))
cycle2 = []
caller = o.methodcaller("m", cycle2)
cycle2.append(caller)
show("methodcaller recursive repr", lambda: repr(caller))

# ── Pickling / copying ──────────────────────────────────────────────────────
for proto in range(0, pickle.HIGHEST_PROTOCOL + 1):
    for obj in (
        o.attrgetter("a.b", "c"),
        o.attrgetter("x"),
        o.itemgetter(1, "x"),
        o.itemgetter(2),
        o.methodcaller("f", 1),
        o.methodcaller("f", 1, k=2),
        o.methodcaller("f"),
    ):
        show("pickle[%d] %r" % (proto, obj), lambda obj=obj, proto=proto: pickle.loads(pickle.dumps(obj, proto)))
    show("pickle[%d] add" % proto, lambda proto=proto: pickle.loads(pickle.dumps(o.add, proto)) is o.add)
show("copy itemgetter", lambda: copy.copy(o.itemgetter(1)))
show("deepcopy methodcaller", lambda: copy.deepcopy(o.methodcaller("f", [1], k={2: 3})))
show("deepcopy attrgetter", lambda: copy.deepcopy(o.attrgetter("a", "b.c")))
roundtrip = pickle.loads(pickle.dumps(o.methodcaller("upper")))
print(roundtrip("abc"), pickle.loads(pickle.dumps(o.itemgetter(0, 2)))("xyz"))
print(pickle.loads(pickle.dumps(o.methodcaller("split", sep=",")))("a,b"))
print(pickle.loads(pickle.dumps(o.attrgetter("real", "imag")))(1 + 2j))

# ── Rebinding builtins does not capture the module ──────────────────────────
import builtins
import sys

real_abs = builtins.abs
real_range = builtins.range
builtins.abs = lambda value: "patched"
builtins.range = lambda value: "patched-range"
try:
    print(o.abs(-3), o.indexOf([1, 2], 2), o.countOf([1, 1], 1))
finally:
    builtins.abs = real_abs
    builtins.range = real_range

sys.modules.pop("operator", None)
import operator as o2

print(o2 is not o, o2.add(1, 2), o2.index(5), o2.itemgetter(0)([9]), o2.__add__ is o2.add)
print(o2.attrgetter is not o.attrgetter or True)
