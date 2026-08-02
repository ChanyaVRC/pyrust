# `operator` module parity (issue #2514).  Exercises the arithmetic /
# bitwise / comparison wrappers, the sequence helpers, `length_hint`, the
# in-place variants, and the generalized lookup objects (`itemgetter`,
# `attrgetter`, `methodcaller`) — including their reprs and error paths.

import operator as o


def show(label, fn):
    try:
        print(label, repr(fn()))
    except Exception as e:
        print(label, type(e).__name__ + ":", str(e))


# Arithmetic / bitwise wrappers.
print(o.add(1, 2), o.sub(5, 3), o.mul(4, 3), o.truediv(7, 2))
print(o.floordiv(7, 2), o.mod(7, 3), o.pow(2, 10), o.abs(-9))
print(o.neg(5), o.pos(-3), o.inv(5), o.invert(5))
print(o.and_(6, 3), o.or_(4, 1), o.xor(5, 1), o.lshift(1, 4), o.rshift(16, 2))

# Comparisons / logical.
print(o.lt(1, 2), o.le(2, 2), o.eq(2, 2), o.ne(2, 3), o.ge(3, 2), o.gt(3, 2))
print(o.not_(0), o.truth([]), o.truth([1]), o.is_(None, None), o.is_not(1, 2))

# Sequence helpers.
print(o.concat([1, 2], [3]), o.contains([1, 2, 3], 2))
print(o.countOf([1, 1, 2, 1], 1), o.indexOf([5, 6, 7], 6))

d = {}
o.setitem(d, "a", 1)
print(o.getitem(d, "a"))
o.delitem(d, "a")
print("a" in d)

# length_hint: exact len, default fallback, and the __length_hint__ protocol.
print(o.length_hint([1, 2, 3]), o.length_hint(object(), 7))


class HasHint:
    def __length_hint__(self):
        return 7


class BadHint:
    def __length_hint__(self):
        return -1


class NotImplHint:
    def __length_hint__(self):
        return NotImplemented


print(o.length_hint(HasHint()), o.length_hint(NotImplHint(), 5))
show("lh-bad", lambda: o.length_hint(BadHint()))
show("lh-default-type", lambda: o.length_hint([1], "x"))

# index follows int.__index__ semantics.
print(o.index(42), o.index(True))
show("index-str", lambda: o.index("x"))

# In-place variants (immutables return a fresh value).
print(o.iadd(1, 2), o.iconcat([1], [2]), o.imul(3, 4), o.isub(10, 3))

# call.
print(o.call(pow, 2, 5))

# itemgetter: single + multiple + string key.
print(o.itemgetter(1)([10, 20, 30]))
print(o.itemgetter(0, 2)([10, 20, 30]))
print(o.itemgetter("k")({"k": "v"}))
print(sorted(["ba", "ab"], key=o.itemgetter(0)))

# attrgetter: single, dotted, and multiple.
class Node:
    pass


n = Node()
n.a = Node()
n.a.b = 42
print(o.attrgetter("a.b")(n))
print(o.attrgetter("a", "a.b")(n) == (n.a, 42))

# methodcaller: bare, with args, with kwargs.
print(list(map(o.methodcaller("lower"), ["HELLO", "World"])))
print(o.methodcaller("count", "l")("hello"))


class Kw:
    def m(self, **kw):
        return kw


print(o.methodcaller("m", name="upper")(Kw()))

# Reprs.
print(repr(o.itemgetter(1)), repr(o.itemgetter(0, 2)))
print(repr(o.attrgetter("x")), repr(o.attrgetter("x", "y")))
print(repr(o.methodcaller("m")), repr(o.methodcaller("m", 1, k=2)))

# Error paths.
show("itemgetter-0", lambda: o.itemgetter())
show("attrgetter-0", lambda: o.attrgetter())
show("methodcaller-0", lambda: o.methodcaller())
show("attrgetter-int", lambda: o.attrgetter(1))
show("methodcaller-int", lambda: o.methodcaller(1))
show("methodcaller-kw-only", lambda: o.methodcaller(name="upper"))
show("indexOf-missing", lambda: o.indexOf([1, 2, 3], 9))
show("concat-nonseq", lambda: o.concat(5, [1]))

# The accelerated operator.index protocol must not capture a user replacement
# for builtins.range (or any other public helper) when operator is imported.
import builtins
import sys


class FakeRangeResult:
    stop = 999


real_range = builtins.range
builtins.range = lambda value: FakeRangeResult()
sys.modules.pop("operator", None)
import operator as rebound_operator

builtins.range = real_range
print("index-rebound-range", rebound_operator.index(5))
show("index-no-args", lambda: rebound_operator.index())
show("index-two-args", lambda: rebound_operator.index(1, 2))
show("index-keyword", lambda: rebound_operator.index(a=1))
print("index-doc", repr(rebound_operator.index.__doc__))
