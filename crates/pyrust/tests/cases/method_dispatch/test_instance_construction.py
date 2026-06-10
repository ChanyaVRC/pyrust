# Instance-construction parity: the single-walk construction plan that
# `instantiate_normal_instance` uses to resolve __new__ / __init__ / primitive
# storage base must behave identically to the per-attribute MRO lookups it
# replaced.  Covers plain classes, default/kw args, __new__ overrides, single-
# and deep-inheritance super().__init__ chains, __slots__, primitive subclasses
# (dict/list/set/int/str/tuple/frozenset), and multiple-inheritance C3 fallback.


# Plain class, no __init__.
class A:
    pass


print(type(A()).__name__)


# __init__ with positional, default, and keyword args.
class B:
    def __init__(self, x, y=10):
        self.x = x
        self.y = y


print(B(1).x, B(1).y, B(1, y=5).y, B(1, 2).y)


# __new__ override that runs before __init__.
class C:
    def __new__(cls, *a):
        o = super().__new__(cls)
        o.tag = "new"
        return o

    def __init__(self, v):
        self.v = v


c = C(7)
print(c.tag, c.v)


# __new__ returning a non-instance skips __init__ (CPython parity).
class H:
    def __new__(cls):
        return 42


print(H())


# Single-inheritance super().__init__ chaining.
class Base:
    def __init__(self, x):
        self.x = x


class Sub(Base):
    def __init__(self, x, y):
        super().__init__(x)
        self.y = y


s = Sub(1, 2)
print(s.x, s.y)


# Deep chain where __init__ is several levels up.
class L1:
    def __init__(self):
        self.n = 1


class L2(L1):
    pass


class L3(L2):
    pass


print(L3().n)


# __slots__ instance.
class G:
    __slots__ = ("a", "b")

    def __init__(self, a, b):
        self.a = a
        self.b = b


g = G(3, 4)
print(g.a, g.b)


# Primitive subclasses: mutable (dict/list/set), scalar (int/str), immutable.
class MyDict(dict):
    def __init__(self):
        super().__init__()
        self["k"] = "v"


print(MyDict()["k"])


class MyList(list):
    def __init__(self, *a):
        super().__init__(a)


print(MyList(1, 2, 3))


class MyInt(int):
    pass


print(MyInt(42) + 1)


class MyStr(str):
    pass


print(MyStr("hi").upper())


class MyTup(tuple):
    pass


print(MyTup((1, 2, 3)))


class MyFro(frozenset):
    pass


print(sorted(MyFro([3, 1, 2, 2])))


# Multiple inheritance: construction plan falls back to the C3 MRO lookup, so
# the inherited __init__ and the C3 method resolution both stay correct.
class M1:
    def __init__(self):
        self.m = 1


class M2:
    pass


class MM(M2, M1):
    pass


print(MM().m)


class P1:
    def foo(self):
        return "P1"


class P2(P1):
    def foo(self):
        return "P2"


class P3(P1):
    def foo(self):
        return "P3"


class Dia(P2, P3):
    def __init__(self):
        self.r = self.foo()


print(Dia().r)


# Metaclass-built class still runs __new__ + __init__ correctly.
class Meta(type):
    pass


class WithMeta(metaclass=Meta):
    def __init__(self):
        self.z = 99


print(WithMeta().z, type(WithMeta).__name__)
