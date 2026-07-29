# Issue #2939: implicit special-method invocation must bind the type-level slot
# through the descriptor protocol, exactly as CPython's `_PyObject_LookupSpecial`
# does.  A `staticmethod` slot is called with no receiver at all; a `classmethod`
# slot receives the owning class in place of the instance.  Both spellings of the
# decorators are covered — the bare `@staticmethod` / `@classmethod` form over a
# Python function and the explicit `staticmethod(value)` / `classmethod(value)`
# wrapper — because pyrust represents them differently internally.
#
# Previously every implicit dunder dispatch prepended the receiver regardless, so
# `__len__ = staticmethod(lambda: 3)` called the lambda with a stray `self` and a
# classmethod dunder saw the instance where CPython passes the class.


def show(label, fn):
    try:
        print(f"{label}: {fn()!r}")
    except Exception as e:
        print(f"{label}: {type(e).__name__}: {e}")


# ── staticmethod-wrapped dunders: called with NO receiver ────────────────────


class SLen:
    __len__ = staticmethod(lambda: 3)


class SBool:
    __bool__ = staticmethod(lambda: False)


class SStr:
    __str__ = staticmethod(lambda: "S")


class SRepr:
    __repr__ = staticmethod(lambda: "R")


class SIter:
    __iter__ = staticmethod(lambda: iter([1, 2]))


class SNext:
    __iter__ = staticmethod(lambda: iter([9]))
    __next__ = staticmethod(lambda: 7)


class SHash:
    __hash__ = staticmethod(lambda: 42)


class SIndex:
    __index__ = staticmethod(lambda: 2)


class SContains:
    __contains__ = staticmethod(lambda x: x == 5)


class SCall:
    __call__ = staticmethod(lambda: "called")


class SGetItem:
    __getitem__ = staticmethod(lambda k: k * 2)


class SEq:
    __eq__ = staticmethod(lambda o: True)


show("static __len__", lambda: len(SLen()))
show("static __bool__", lambda: bool(SBool()))
show("static __str__", lambda: str(SStr()))
show("static __repr__", lambda: repr(SRepr()))
show("static __iter__", lambda: list(SIter()))
show("static __next__", lambda: next(SNext()))
show("static __hash__", lambda: hash(SHash()))
show("static __index__", lambda: [10, 20, 30][SIndex()])
show("static __contains__", lambda: 5 in SContains())
show("static __call__", lambda: SCall()())
show("static __getitem__", lambda: SGetItem()[21])
show("static __eq__", lambda: SEq() == 1)


# The decorator form over a plain `def` must behave identically.


class DecLen:
    @staticmethod
    def __len__():
        return 3


class DecGetItem:
    @staticmethod
    def __getitem__(k):
        return ("dec", k)


class DecCall:
    @staticmethod
    def __call__(*a):
        return ("deccall", a)


show("decorated static __len__", lambda: len(DecLen()))
show("decorated static __getitem__", lambda: DecGetItem()[8])
show("decorated static __call__", lambda: DecCall()(1, 2))


# ── classmethod-wrapped dunders: receive the class, not the instance ─────────


class CLen:
    @classmethod
    def __len__(cls):
        return len(cls.__name__)


class CGetItem:
    @classmethod
    def __getitem__(cls, k):
        return (cls.__name__, k)


class CStr:
    @classmethod
    def __str__(cls):
        return "C:" + cls.__name__


class CCall:
    @classmethod
    def __call__(cls):
        return "cc:" + cls.__name__


class CContains:
    @classmethod
    def __contains__(cls, item):
        return item == cls.__name__


show("cls __len__", lambda: len(CLen()))
show("cls __getitem__", lambda: CGetItem()[3])
show("cls __str__", lambda: str(CStr()))
show("cls __call__", lambda: CCall()())
show("cls __contains__", lambda: "CContains" in CContains())


# A classmethod dunder binds `type(instance)`, so a subclass sees itself.


class CBase:
    @classmethod
    def __len__(cls):
        return len(cls.__name__)


class CDerived(CBase):
    pass


show("cls __len__ via subclass", lambda: len(CDerived()))


# ── inheritance ─────────────────────────────────────────────────────────────


class SBase:
    @staticmethod
    def __len__():
        return 11


class SInherit(SBase):
    pass


class SOverride(SBase):
    def __len__(self):
        return 22


show("inherited static __len__", lambda: len(SInherit()))
show("plain override of static base", lambda: len(SOverride()))


# ── metaclass-provided dunders on the class object itself ───────────────────


class MetaStaticLen(type):
    __len__ = staticmethod(lambda: 77)


class WithMetaStaticLen(metaclass=MetaStaticLen):
    pass


class MetaStaticCall(type):
    __call__ = staticmethod(lambda *a: ("meta-static-call", a))


class WithMetaStaticCall(metaclass=MetaStaticCall):
    pass


show("metaclass static __len__", lambda: len(WithMetaStaticLen))
show("metaclass static __call__", lambda: WithMetaStaticCall())


# ── arity diagnostics still name the underlying callable ────────────────────


class ArityStatic:
    __len__ = staticmethod(lambda self: 3)


class ArityPlain:
    def __len__():
        return 3


show("static arity error", lambda: len(ArityStatic()))
show("plain arity error", lambda: len(ArityPlain()))


# ── non-descriptor slots keep their existing no-receiver behaviour ──────────
# A `functools.partial` is not a descriptor, so CPython calls it without `self`.

import functools


class PartialLen:
    __len__ = functools.partial(lambda a: a, 5)


class PartialGetItem:
    __getitem__ = functools.partial(lambda tag, k: (tag, k), "T")


show("partial __len__", lambda: len(PartialLen()))
show("partial __getitem__", lambda: PartialGetItem()[1])


# ── explicit attribute access is unaffected ─────────────────────────────────


class Explicit:
    @classmethod
    def cm(cls):
        return cls.__name__

    @staticmethod
    def sm():
        return "sm"


print(Explicit.cm(), Explicit().cm(), Explicit.sm(), Explicit().sm())


# ── the sibling dispatch paths ──────────────────────────────────────────────
# Binary / reflected / in-place operators, the unary and numeric-conversion
# slots, the item and attribute protocols, the context-manager protocol and the
# `isinstance` / `issubclass` metaclass hooks each have their own dispatch site.
# They must all agree with the generic one.


class Add:
    __add__ = staticmethod(lambda o: ("add", o))


class Lt:
    __lt__ = staticmethod(lambda o: ("lt", o))


class RAdd:
    __radd__ = staticmethod(lambda o: ("radd", o))


class IAdd:
    __iadd__ = staticmethod(lambda o: ("iadd", o))


class Neg:
    __neg__ = staticmethod(lambda: "neg")


class Int_:
    __int__ = staticmethod(lambda: 5)


class CAdd:
    @classmethod
    def __add__(cls, o):
        return (cls.__name__, "add", o)


class CRAdd:
    @classmethod
    def __radd__(cls, o):
        return (cls.__name__, "radd", o)


def do_iadd():
    x = IAdd()
    x += 5
    return x


show("static __add__", lambda: Add() + 1)
show("static __lt__", lambda: Lt() < 1)
show("static __radd__", lambda: 1 + RAdd())
show("static __iadd__", do_iadd)
show("static __neg__", lambda: -Neg())
show("static __int__", lambda: int(Int_()))
show("cls __add__", lambda: CAdd() + 7)
show("cls __radd__", lambda: 1 + CRAdd())


class SetItem:
    __setitem__ = staticmethod(lambda k, v: ("setitem", k, v))


class GetAttrS:
    __getattr__ = staticmethod(lambda name: ("getattr", name))


class CGetAttr:
    @classmethod
    def __getattr__(cls, name):
        return (cls.__name__, name)


class Ctx:
    __enter__ = staticmethod(lambda: "entered")
    __exit__ = staticmethod(lambda *a: False)


class Fmt:
    __format__ = staticmethod(lambda spec: "FMT:" + spec)


class Rev:
    __reversed__ = staticmethod(lambda: iter([3, 2, 1]))


def do_setitem():
    obj = SetItem()
    obj[1] = 2
    return "ok"


def do_ctx():
    with Ctx() as v:
        return v


show("static __setitem__", do_setitem)
show("static __getattr__", lambda: GetAttrS().missing)
show("cls __getattr__", lambda: CGetAttr().nope)
show("static __enter__/__exit__", do_ctx)
show("static __format__", lambda: format(Fmt(), "x"))
show("static __reversed__", lambda: list(reversed(Rev())))


class MetaInstanceCheck(type):
    __instancecheck__ = staticmethod(lambda inst: True)


class UsesInstanceCheck(metaclass=MetaInstanceCheck):
    pass


class MetaSubclassCheck(type):
    __subclasscheck__ = staticmethod(lambda sub: True)


class UsesSubclassCheck(metaclass=MetaSubclassCheck):
    pass


show("static __instancecheck__", lambda: isinstance(42, UsesInstanceCheck))
show("static __subclasscheck__", lambda: issubclass(int, UsesSubclassCheck))


# ── plain-function dunders (the common case) are untouched ──────────────────


class Plain:
    def __len__(self):
        return 1

    def __getitem__(self, k):
        return k * 3

    def __eq__(self, o):
        return True

    def __hash__(self):
        return 5


p = Plain()
print(len(p), p[4], p == 1, hash(p))
