# Parity fixture for #2392 — the CallMethodKw fast-bind path for keyword-argument
# method calls (`obj.m(a=1, b=2)`).  The receiver binds to parameter 0 and a
# per-call-site cache (shared with #2382's CallKw) maps the keyword names to
# parameter slots, so the call binds with no dict/list build and no name scan.
# The general method binder still owns every CPython-parity diagnostic.
#
# Exercise the full binding matrix — defaults, keyword-only, positional-only,
# reordered keywords, inheritance + override, classmethod/staticmethod, and a
# polymorphic receiver through one call site (cache-guard correctness) — so any
# wrong slot, stale cache, or wrong receiver diverges from CPython.


class C:
    def m(self, a, b, c):
        return (a, b, c)

    def md(self, a, b=10, c=20):
        return (a, b, c)

    def konly(self, a, *, b, c=99):
        return (a, b, c)

    def ponly(self, a, b, /, c):
        return (a, b, c)


o = C()

# Plain keyword binding, mixed positional/keyword, and reordered keywords.
print(o.m(a=1, b=2, c=3))
print(o.m(1, b=2, c=3))
print(o.m(1, 2, c=3))
print(o.m(c=3, a=1, b=2))

# Defaults: filled, overridden by keyword, and bound entirely by keyword.
print(o.md(1))
print(o.md(1, c=99))
print(o.md(a=1, b=2))

# Keyword-only parameters.
print(o.konly(1, b=2))
print(o.konly(1, b=2, c=3))
print(o.konly(a=1, b=2, c=3))

# Positional-only parameter with a trailing keyword.
print(o.ponly(1, 2, c=3))
print(o.ponly(1, 2, 3))


# Inheritance: inherited method and an overriding subclass, both via kwargs.
class Base:
    def f(self, x, y):
        return ("base", x, y)


class Inherit(Base):
    pass


class Override(Base):
    def f(self, x, y):
        return ("over", x, y)


print(Inherit().f(x=1, y=2))
print(Override().f(x=1, y=2))


# classmethod / staticmethod with keyword arguments, via class and via instance.
class D:
    @classmethod
    def cm(cls, a, b):
        return (cls.__name__, a, b)

    @staticmethod
    def sm(a, b):
        return ("sm", a, b)


print(D.cm(a=1, b=2))
print(D().cm(a=5, b=6))
print(D.sm(a=1, b=2))
print(D().sm(a=7, b=8))


# Polymorphic receiver through a single call site: the cache guard
# (`param_binds` identity + class version) must keep the two classes' methods
# distinct even though the method name and keyword shape are identical.
class P1:
    def g(self, a):
        return ("p1", a)


class P2:
    def g(self, a):
        return ("p2", a)


for obj in (P1(), P2(), P1(), P2(), P1()):
    print(obj.g(a=42))


# A method whose receiver is mutated in place through a keyword-only call,
# repeated, to confirm the receiver binds to `self` (not dropped) every call.
class Counter:
    def __init__(self):
        self.n = 0

    def add(self, *, by):
        self.n += by
        return self.n


ctr = Counter()
print(ctr.add(by=5))
print(ctr.add(by=3))
print(ctr.n)


# Non-fast-local receiver (attribute chain) so the receiver is copied into a
# temp register rather than reusing a local — same binding must hold.
class Holder:
    def __init__(self):
        self.c = C()


h = Holder()
print(h.c.m(a=7, b=8, c=9))
print(h.c.md(1, b=2))
