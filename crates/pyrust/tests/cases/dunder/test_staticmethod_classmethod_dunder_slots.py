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


# ── `__get__` is the ONE slot CPython does not descriptor-bind ───────────────
# `slot_tp_descr_get` resolves `__get__` with a raw `_PyType_Lookup` and calls
# it directly as `get(self, obj, objtype)`, so a staticmethod `__get__` still
# receives the descriptor positionally and a classmethod `__get__` is simply not
# callable.  `__set__` / `__delete__` go through `vectorcall_method` and DO bind
# — the asymmetry is deliberate and is what this section pins down.


class GetStatic3:
    @staticmethod
    def __get__(desc, obj, objtype):
        return ("g3", type(desc).__name__, obj is None, objtype.__name__)


class UsesGetStatic3:
    d = GetStatic3()


class GetStatic2:
    @staticmethod
    def __get__(obj, objtype=None):
        return "g2"


class UsesGetStatic2:
    d = GetStatic2()


class GetClass:
    @classmethod
    def __get__(cls, obj, objtype=None):
        return "gc"


class UsesGetClass:
    d = GetClass()


class GetPlain:
    def __get__(self, obj, objtype=None):
        return ("gp", obj is None)


class UsesGetPlain:
    d = GetPlain()


show("static __get__/3 instance", lambda: UsesGetStatic3().d)
show("static __get__/3 class", lambda: UsesGetStatic3.d)
show("static __get__/2 (raw call passes 3)", lambda: UsesGetStatic2().d)
show("cls __get__ (not callable)", lambda: UsesGetClass().d)
show("plain __get__ instance", lambda: UsesGetPlain().d)
show("plain __get__ class", lambda: UsesGetPlain.d)


class SetStatic:
    @staticmethod
    def __get__(desc, obj, objtype):
        return "sget"

    @staticmethod
    def __set__(obj, value):
        print("static __set__ value:", value)

    @staticmethod
    def __delete__(obj):
        print("static __delete__ ran")


class UsesSetStatic:
    d = SetStatic()


class SetCls:
    @staticmethod
    def __get__(desc, obj, objtype):
        return "cget"

    @classmethod
    def __set__(cls, obj, value):
        print("cls __set__ cls:", cls.__name__, "value:", value)


class UsesSetCls:
    d = SetCls()


def do_static_set():
    u = UsesSetStatic()
    u.d = 1
    del u.d
    return "ok"


def do_cls_set():
    u = UsesSetCls()
    u.d = 2
    return "ok"


show("static __set__/__delete__ bind", do_static_set)
show("cls __set__ binds owner", do_cls_set)


# ── copy.copy passes the object explicitly; copy.deepcopy does not ──────────
# `copy.copy` is pure Python: `copier = getattr(cls, "__copy__"); copier(x)`.
# The class-level getattr applies the descriptor binding AND `x` is still passed
# as a real argument, so a staticmethod `__copy__` needs one parameter and a
# classmethod one needs two.  `copy.deepcopy` uses an *instance* getattr and
# passes only `memo`.
import copy as _copy


class CopyStatic0:
    @staticmethod
    def __copy__():
        return "cs0"


class CopyStatic1:
    @staticmethod
    def __copy__(x):
        return "cs1"


class CopyCls0:
    @classmethod
    def __copy__(cls):
        return "cc0"


class CopyCls1:
    @classmethod
    def __copy__(cls, x):
        return "cc1:" + cls.__name__


class CopyPlain:
    def __copy__(self):
        return "cp"


class DeepStatic1:
    @staticmethod
    def __deepcopy__(memo):
        return "ds1"


class DeepStatic2:
    @staticmethod
    def __deepcopy__(x, memo):
        return "ds2"


class DeepCls:
    @classmethod
    def __deepcopy__(cls, memo):
        return "dc:" + cls.__name__


show("copy static/0", lambda: _copy.copy(CopyStatic0()))
show("copy static/1", lambda: _copy.copy(CopyStatic1()))
show("copy cls/0", lambda: _copy.copy(CopyCls0()))
show("copy cls/1", lambda: _copy.copy(CopyCls1()))
show("copy plain", lambda: _copy.copy(CopyPlain()))
show("deepcopy static/1", lambda: _copy.deepcopy(DeepStatic1()))
show("deepcopy static/2", lambda: _copy.deepcopy(DeepStatic2()))
show("deepcopy cls", lambda: _copy.deepcopy(DeepCls()))


# `type.__call__` supplies the constructed class explicitly to `__new__`.
# A classmethod binds that class as well, so its body observes two class args.


class NewRegular:
    def __new__(cls, tag):
        obj = object.__new__(cls)
        obj.seen = (cls.__name__, tag)
        return obj


class NewStatic:
    @staticmethod
    def __new__(cls, tag):
        obj = object.__new__(cls)
        obj.seen = (cls.__name__, tag)
        return obj


class NewClass:
    @classmethod
    def __new__(bound_cls, explicit_cls, tag):
        obj = object.__new__(explicit_cls)
        obj.seen = (bound_cls.__name__, explicit_cls.__name__, tag)
        return obj


class NewClassChild(NewClass):
    pass


class NewClassArity:
    @classmethod
    def __new__(cls):
        return object.__new__(cls)


class NewClassException(Exception):
    @classmethod
    def __new__(bound_cls, explicit_cls, tag):
        obj = super().__new__(explicit_cls, tag)
        obj.seen = (bound_cls.__name__, explicit_cls.__name__, tag)
        return obj


show("regular __new__ cls", lambda: NewRegular("r").seen)
show("static __new__ cls", lambda: NewStatic("s").seen)
show("cls __new__ cls twice", lambda: NewClass("c").seen)
show("inherited cls __new__ cls twice", lambda: NewClassChild("i").seen)
show("cls __new__ arity", lambda: NewClassArity())
show("exception cls __new__ cls twice", lambda: NewClassException("e").seen)


# copy.copy reaches a class-level staticmethod __reduce__ through the inherited
# object.__reduce_ex__(4) fallback. Keep custom __reduce_ex__ outside this
# issue: the control below asserts only that the narrow shortcut does not steal
# its precedence.


class ReduceTarget:
    def __init__(self, value):
        self.value = value


class ReduceStatic:
    @staticmethod
    def __reduce__():
        return (ReduceTarget, (7,), {"extra": 8})


class ReduceStaticChild(ReduceStatic):
    pass


class ReduceStaticException(Exception):
    @staticmethod
    def __reduce__():
        return "identity"


class ReduceIdentity:
    @staticmethod
    def __reduce__():
        return "identity"


class ReduceBad:
    @staticmethod
    def __reduce__():
        return 1


class RVSame:
    def __iter__(self):
        raise TypeError("'RVSame' object is not iterable")


class ReduceIterError:
    @staticmethod
    def __reduce__():
        return RVSame()


class RVNoneIter:
    __iter__ = None


class ReduceNoneIter:
    @staticmethod
    def __reduce__():
        return RVNoneIter()


class ReduceTupleIterator:
    @staticmethod
    def __reduce__():
        return iter((ReduceTarget, (11,), {"extra": 12}))


class ReduceGenerator:
    @staticmethod
    def __reduce__():
        return (item for item in (ReduceTarget, (13,), {"extra": 14}))


class ReduceBuiltinFunction:
    @staticmethod
    def __reduce__():
        return len


async def reduction_coroutine_value():
    return None


async def reduction_async_generator_value():
    yield None


reduction_coroutines = []


class ReduceCoroutine:
    @staticmethod
    def __reduce__():
        value = reduction_coroutine_value()
        reduction_coroutines.append(value)
        return value


class ReduceAsyncGenerator:
    @staticmethod
    def __reduce__():
        return reduction_async_generator_value()


class ReductionMeta(type):
    def __getattribute__(cls, name):
        if name in {"__mro__", "__dict__", "__name__"}:
            raise AssertionError(f"metaclass lookup leaked: {name}")
        return super().__getattribute__(name)


class RVMeta(metaclass=ReductionMeta):
    def __iter__(self):
        yield from (ReduceTarget, (15,), {"extra": 16})


class ReduceMetaIterable:
    @staticmethod
    def __reduce__():
        return RVMeta()


class CopyBeforeReduce:
    @staticmethod
    def __copy__(obj):
        return "copy"

    @staticmethod
    def __reduce__():
        return "reduce"


class NoneCopyBeforeReduce:
    __copy__ = None

    @staticmethod
    def __reduce__():
        return "identity"


class ReduceExNone:
    __reduce_ex__ = None

    @staticmethod
    def __reduce__():
        return "identity"


class ReduceExStaticNone:
    __reduce_ex__ = staticmethod(None)

    @staticmethod
    def __reduce__():
        return "identity"


reduce_precedence_events = []


class ReduceExPrecedence:
    def __reduce_ex__(self, protocol):
        reduce_precedence_events.append("reduce_ex")
        return "custom"

    @staticmethod
    def __reduce__():
        reduce_precedence_events.append("static")
        return "static"


def copied_static_reduce(cls):
    obj = _copy.copy(cls())
    return (type(obj).__name__, obj.value, obj.extra)


def static_reduce_identity():
    obj = ReduceIdentity()
    return _copy.copy(obj) is obj


def exception_static_reduce_identity():
    obj = ReduceStaticException("value")
    return _copy.copy(obj) is obj


def reduction_error_details(cls):
    try:
        _copy.copy(cls())
    except TypeError as error:
        return (str(error), error.__context__ is None)


def custom_reduce_ex_keeps_precedence():
    reduce_precedence_events.clear()
    _copy.copy(ReduceExPrecedence())
    return "static" not in reduce_precedence_events


def none_copy_falls_through():
    obj = NoneCopyBeforeReduce()
    return _copy.copy(obj) is obj


def reduce_ex_none_falls_through(cls):
    obj = cls()
    return _copy.copy(obj) is obj


def explicit_static_reduce():
    return (ReduceTarget, (9,), {"extra": 10})


class ReduceStaticExplicit:
    __reduce__ = staticmethod(explicit_static_reduce)


show("copy static __reduce__ reconstruct", lambda: copied_static_reduce(ReduceStatic))
show(
    "copy inherited static __reduce__ reconstruct",
    lambda: copied_static_reduce(ReduceStaticChild),
)
show(
    "copy exception static __reduce__ identity",
    exception_static_reduce_identity,
)
show("copy static __reduce__ identity", static_reduce_identity)
show("copy static __reduce__ malformed", lambda: reduction_error_details(ReduceBad))
show("copy static __reduce__ iterator error", lambda: reduction_error_details(ReduceIterError))
show("copy static __reduce__ None iter", lambda: reduction_error_details(ReduceNoneIter))
show("copy static __reduce__ tuple iterator", lambda: copied_static_reduce(ReduceTupleIterator))
show("copy static __reduce__ generator", lambda: copied_static_reduce(ReduceGenerator))
show(
    "copy static __reduce__ builtin function",
    lambda: reduction_error_details(ReduceBuiltinFunction),
)
show(
    "copy static __reduce__ coroutine",
    lambda: reduction_error_details(ReduceCoroutine),
)
reduction_coroutines.pop().close()
show(
    "copy static __reduce__ async generator",
    lambda: reduction_error_details(ReduceAsyncGenerator),
)
show(
    "copy static __reduce__ metaclass iterable",
    lambda: copied_static_reduce(ReduceMetaIterable),
)
show("copy __copy__ before static __reduce__", lambda: _copy.copy(CopyBeforeReduce()))
show("copy None __copy__ falls through", none_copy_falls_through)
show(
    "copy None __reduce_ex__ falls through",
    lambda: reduce_ex_none_falls_through(ReduceExNone),
)
show(
    "copy static None __reduce_ex__ falls through",
    lambda: reduce_ex_none_falls_through(ReduceExStaticNone),
)
show("copy custom __reduce_ex__ precedence", custom_reduce_ex_keeps_precedence)
show(
    "copy explicit static __reduce__ reconstruct",
    lambda: copied_static_reduce(ReduceStaticExplicit),
)
