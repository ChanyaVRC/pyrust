# Issue #3331: plain methods, descriptors, dunder/sunder overrides, `nonmember`
# and methods inherited from member-less base enums must not become members.
import functools
from enum import Enum, Flag, IntEnum, StrEnum, member, nonmember, unique


class E(Enum):
    A = 1
    B = 2

    def label(self):
        return self.name.lower()

    @property
    def doubled(self):
        return self.value * 2

    @classmethod
    def names(cls):
        return [m.name for m in cls]

    @staticmethod
    def helper(x):
        return x + 1

    def __str__(self):
        return f"E:{self.name}"

    def __repr__(self):
        return f"<E {self.name}>"

    def __format__(self, spec):
        return "fmt-" + format(self.name, spec)

    plain_non = nonmember(42)
    square = lambda self: self.value ** 2


print(list(E), len(E), E._member_names_)
print(E.A.label(), E.B.doubled, E.names(), E.helper(5), E.A.helper(6))
print(str(E.A), repr(E.A), f"{E.A:>3}", format(E.B, ""))
print(E.plain_non, E.A.plain_non, "plain_non" in E._member_map_)
print(E.A.square(), E.B.square())
print(E["A"] is E.A, E(2) is E.B, E.A.name, E.A.value)
print("label" in E._member_map_, "doubled" in E._member_map_, "names" in E._member_map_)
print(type(E.__dict__["label"]).__name__, type(E.__dict__["doubled"]).__name__)
print(type(E.__dict__["names"]).__name__, type(E.__dict__["helper"]).__name__)
try:
    E("label")
except ValueError as exc:
    print("ValueError", exc)


# Descriptors: custom get / set / delete, cached_property.
class Getter:
    def __get__(self, obj, owner=None):
        return "get"


class Setter:
    def __set__(self, obj, value):
        pass


class Deleter:
    def __delete__(self, obj):
        pass


class D(Enum):
    X = 1
    getter = Getter()
    setter = Setter()
    deleter = Deleter()

    @functools.cached_property
    def cached(self):
        return self.value + 100


print(list(D), D.X.cached, D.X.getter)
print([n for n in D._member_map_])


# Underscore / dunder / sunder names are never members.
class U(Enum):
    A = 1
    _ignore_me = 5
    __dun__ = 8

    def _helper(self):
        return "h"


print(list(U), U.A._helper(), U._ignore_me, U.__dun__)


# `member` forces a member, `nonmember` forces a plain attribute.
class M(Enum):
    A = 1
    f = member(lambda: "called")
    g = nonmember(lambda: "plain")


print([m.name for m in M], M.f.value(), M.g())
print(M.f.name, M(1) is M.A)


# Methods inherited from a member-less base enum.
class Base(Enum):
    def hello(self):
        return f"hi {self.name}"

    @property
    def shout(self):
        return self.name.upper() + "!"

    @classmethod
    def count(cls):
        return len(cls)

    @staticmethod
    def twice(x):
        return x * 2

    def __str__(self):
        return f"Base<{self.name}>"


class Sub(Base):
    X = 1
    Y = 2

    def own(self):
        return "own"


print(list(Sub), Sub.X.hello(), Sub.Y.shout, Sub.count(), Sub.twice(4), Sub.X.own())
print(str(Sub.X), repr(Sub.Y), Sub.X in Sub)


class Sub2(Sub.__mro__[1]):
    P = "p"

    def hello(self):
        return "override " + super().hello()


print(list(Sub2), Sub2.P.hello())


# Mixins and builtin-backed enums.
class I(IntEnum):
    ONE = 1
    TWO = 2

    def is_one(self):
        return self is I.ONE

    @property
    def neg(self):
        return -self.value

    @classmethod
    def total(cls):
        return sum(cls)

    @staticmethod
    def inc(x):
        return x + 1


print(list(I), I.ONE.is_one(), I.TWO.neg, I.total(), I.inc(1), I.TWO + 1)


class S(StrEnum):
    LOW = "low"
    HIGH = "high"

    def shout(self):
        return self.upper() + "!"

    @property
    def size(self):
        return len(self)

    @classmethod
    def first(cls):
        return next(iter(cls))


print(list(S), S.LOW.shout(), S.HIGH.size, S.first(), str(S.LOW))


class F(Flag):
    R = 1
    W = 2
    X = 4

    def has_w(self):
        return bool(self & F.W)

    @property
    def count(self):
        return bin(self.value).count("1")

    @classmethod
    def everything(cls):
        return cls.R | cls.W | cls.X

    @staticmethod
    def mask():
        return 7


print(list(F), (F.R | F.W).has_w(), F.R.has_w(), (F.R | F.X).count)
print(F.everything(), F.mask(), F._member_map_.keys())


@unique
class Uniq(Enum):
    A = 1
    B = 2

    def method(self):
        return self.name


print(list(Uniq), Uniq.A.method())


# Functional API.
Func = Enum("Func", {"A": 1, "B": 2})
print(list(Func))


# Methods defined after members, and a method named like a member of a base.
class Late(Enum):
    ONE = 1

    def f(self):
        return 1

    TWO = 2

    def g(self):
        return 2


print(list(Late), Late.TWO.f(), Late.ONE.g())
