# Issue #3293: `abc` is implemented natively in Rust (no `abc_py.py`).  Every
# observable behaviour below must match CPython 3.12, whose `abc` is a thin
# Python layer over the C `_abc` accelerator: the class layout (`_abc_impl`,
# no `ABCMeta.__call__`), abstract-instantiation errors raised by
# `object.__new__`, the weak virtual-subclass registry with its positive and
# negative caches, cache-token invalidation, `__subclasshook__` dispatch,
# cooperative metaclass `__new__`, and `update_abstractmethods`.

import abc
import sys
from abc import ABC, ABCMeta, abstractmethod


def show(label, fn):
    try:
        print(label, fn())
    except Exception as e:
        print(label, type(e).__name__, e)


print(type(ABC), type(ABCMeta), ABC.__mro__, ABCMeta.__mro__)
print(list(vars(ABC)), list(vars(ABCMeta)))
print(ABC.__slots__, ABC.__abstractmethods__, ABC.__doc__ == "Helper class that provides a standard way to create an ABC using\n    inheritance.\n    ")
print(ABCMeta.__doc__[:40], ABCMeta.__module__, ABC.__module__, ABCMeta.__name__, ABCMeta.__qualname__)
print(sorted(n for n in dir(abc) if not n.startswith('_')))


class Shape(ABC):
    @abstractmethod
    def area(self): ...

    @property
    @abstractmethod
    def name(self): ...

    @classmethod
    @abstractmethod
    def make(cls): ...

    @staticmethod
    @abstractmethod
    def util(): ...


print(sorted(Shape.__abstractmethods__), type(Shape.__abstractmethods__).__name__)
print(list(vars(Shape))[-2:], type(Shape._abc_impl).__name__, type(Shape._abc_impl).__module__)
show("inst", lambda: Shape())


class Half(Shape):
    def area(self): return 1


show("half", lambda: Half())
print(sorted(Half.__abstractmethods__))


class Full(Half):
    name = "x"
    @classmethod
    def make(cls): return cls()
    @staticmethod
    def util(): return 1


print(Full().area(), Full.__abstractmethods__, Full.make().name)


class One(ABC):
    @abstractmethod
    def f(self): ...


show("one", lambda: One())

# register / virtual subclasses
class Base(ABC):
    pass


class Virt:
    pass


tok = abc.get_cache_token()
print(Base.register(Virt) is Virt, abc.get_cache_token() == tok + 1)
print(issubclass(Virt, Base), isinstance(Virt(), Base), issubclass(int, Base), isinstance(3, Base))


class VirtSub(Virt):
    pass


print(issubclass(VirtSub, Base), isinstance(VirtSub(), Base))
show("reg-nonclass", lambda: Base.register(3))
show("reg-cycle", lambda: Virt.register if False else Base.register(Base))


class Sub(Base):
    pass


show("cycle", lambda: Sub.register(Base))
print(issubclass(Sub, Base), issubclass(Base, Sub))

# decorator usage
@Base.register
class Deco:
    pass


print(isinstance(Deco(), Base))


# negative cache invalidation
class Late:
    pass


print(issubclass(Late, Base))
Base.register(Late)
print(issubclass(Late, Base))


# subclasshook
class Sized2(ABC):
    @classmethod
    def __subclasshook__(cls, C):
        if cls is Sized2:
            if any("__len__" in B.__dict__ for B in C.__mro__):
                return True
        return NotImplemented


class HasLen:
    def __len__(self): return 0


print(issubclass(HasLen, Sized2), isinstance(HasLen(), Sized2), issubclass(int, Sized2), isinstance([], Sized2))


class BadHook(ABC):
    @classmethod
    def __subclasshook__(cls, C):
        return 1


show("badhook", lambda: issubclass(int, BadHook))
show("subcheck-nonclass", lambda: issubclass(3, Base))
show("subcheck-direct", lambda: Base.__subclasscheck__(3))

# transitive through subclass registry
class Number(ABC): pass
class Complex(Number): pass
class Real(Complex): pass
Real.register(float)
print(issubclass(float, Number), isinstance(1.5, Number), isinstance(1, Number), issubclass(bool, Number))

# metaclass keyword and custom metaclass
class M(ABCMeta):
    def __new__(mcls, name, bases, ns, **kw):
        cls = super().__new__(mcls, name, bases, ns, **kw)
        cls.tag = name.lower()
        return cls


class WithM(metaclass=M):
    @abstractmethod
    def g(self): ...


print(WithM.tag, sorted(WithM.__abstractmethods__), type(WithM).__name__)
show("withm", lambda: WithM())


class Plain(metaclass=ABCMeta):
    pass


print(Plain(), Plain.__abstractmethods__) if False else print(type(Plain()).__name__, Plain.__abstractmethods__)

# update_abstractmethods
class U(ABC):
    @abstractmethod
    def h(self): ...


class V(U):
    pass


V.h = lambda self: 1
show("v-before", lambda: V())
print(abc.update_abstractmethods(V) is V, V.__abstractmethods__)
show("v-after", lambda: V().h())
print(abc.update_abstractmethods(int) is int)

# deprecated decorators
class D(ABC):
    @abc.abstractclassmethod
    def c(cls): ...
    @abc.abstractstaticmethod
    def s(): ...
    @abc.abstractproperty
    def p(self): ...


print(sorted(D.__abstractmethods__))
show("d", lambda: D())

# abstractmethod flag
def fn(): pass
print(abstractmethod(fn) is fn, fn.__isabstractmethod__)

# __init_subclass__ kwargs pass through
class K(ABC):
    def __init_subclass__(cls, flag=None, **kw):
        super().__init_subclass__(**kw)
        cls.flag = flag


class K2(K, flag=7):
    pass


print(K2.flag)

# __slots__ behaviour
class Slotted(ABC):
    __slots__ = ("a",)


s = Slotted(); s.a = 1
show("slot-dict", lambda: s.__dict__)
show("slot-set", lambda: setattr(s, "b", 2))


class Unslotted(ABC):
    pass


u = Unslotted(); u.z = 3; print(u.__dict__)

# manual __abstractmethods__ on an ABCMeta class
class Manual(ABC):
    pass


Manual.__abstractmethods__ = frozenset({"q", "r"})
show("manual", lambda: Manual())
Manual.__abstractmethods__ = frozenset()
print(type(Manual()).__name__)

# registry dump shape
import io
buf = io.StringIO()
Base._dump_registry(file=buf)
lines = buf.getvalue().splitlines()
print(lines[0], [l.split(":")[0] for l in lines])
Base._abc_caches_clear(); Base._abc_registry_clear()
print(issubclass(Virt, Base), issubclass(Sub, Base))

# isinstance consults both __class__ and type() of the instance
class Virt2: pass
class Proxy:
    __class__ = Virt2
class B2(ABC): pass
B2.register(Virt2)
print(issubclass(Proxy, B2), issubclass(Virt2, B2), isinstance(Virt2(), B2))

# multiple inheritance metaclass
class Other(type):
    def __new__(mcls, name, bases, ns, **kw):
        ns = dict(ns); ns["other"] = True
        return super().__new__(mcls, name, bases, ns, **kw)


class Both(ABCMeta, Other):
    pass


class UsesBoth(metaclass=Both):
    @abstractmethod
    def m(self): ...


print(UsesBoth.other, sorted(UsesBoth.__abstractmethods__))

import os, collections.abc, numbers, io as _io, typing
print(type(os.PathLike).__name__, issubclass(str, os.PathLike), sorted(os.PathLike.__abstractmethods__))
show("pathlike", lambda: os.PathLike())


class P:
    def __fspath__(self): return "x"


print(isinstance(P(), os.PathLike), issubclass(P, os.PathLike), os.fspath(P()))
print(isinstance(3, numbers.Integral), isinstance(3.0, numbers.Real), isinstance(3.0, numbers.Integral), issubclass(bool, numbers.Number))
print(isinstance([], collections.abc.Sequence), isinstance({}, collections.abc.Mapping), issubclass(dict, collections.abc.MutableMapping))
print(isinstance(_io.StringIO(), _io.IOBase), issubclass(_io.BytesIO, _io.BufferedIOBase))
print('abc' in sys.modules)
