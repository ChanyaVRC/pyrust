"""A descriptor in a special-method slot is bound through `__get__` (issue #2944).

CPython's slot lookup (`_PyObject_LookupSpecial` / `lookup_maybe_method`) is
uniform: after `_PyType_Lookup` finds the slot it consults the found object's
`tp_descr_get`, calls `__get__(instance, type(instance))`, and then calls the
*result* with the slot's own arguments.  A `property` is simply one such
descriptor, so `__len__ = property(fget)` runs `fget(obj)` and calls what it
returns -- meaning a getter returning a non-callable raises
`TypeError: 'int' object is not callable`, naming the getter's result.

The one deliberate exception is `__get__` itself: `slot_tp_descr_get` resolves
it with a raw `_PyType_Lookup` and calls it directly, never binding it.
"""

import functools


def t(label, fn):
    try:
        print(f"{label}: OK {fn()!r}")
    except BaseException as e:  # noqa: BLE001 - parity fixture reports the class
        print(f"{label}: {type(e).__name__}: {e}")


# --- property in a slot: the getter runs, its result is called -------------
class PropLen:
    @property
    def __len__(self):
        print("  [PropLen getter ran]")
        return lambda: 4


t("property -> callable", lambda: len(PropLen()))


class PropLenValue:
    @property
    def __len__(self):
        print("  [PropLenValue getter ran]")
        return 4


# The getter still runs; the error names `int`, not `property`.
t("property -> non-callable", lambda: len(PropLenValue()))


# --- across the dunder family ---------------------------------------------
class PropStr:
    @property
    def __str__(self):
        return lambda: "PROP"


t("property __str__", lambda: str(PropStr()))


class PropRepr:
    @property
    def __repr__(self):
        return lambda: "PREPR"


t("property __repr__", lambda: repr(PropRepr()))


class PropBool:
    @property
    def __bool__(self):
        return lambda: False


t("property __bool__", lambda: bool(PropBool()))


class PropIter:
    @property
    def __iter__(self):
        return lambda: iter([1, 2, 3])


t("property __iter__", lambda: list(PropIter()))


class PropHash:
    @property
    def __hash__(self):
        return lambda: 99


t("property __hash__", lambda: hash(PropHash()))


class PropContains:
    @property
    def __contains__(self):
        return lambda x: x == 5


t("property __contains__", lambda: 5 in PropContains())


class PropGetItem:
    @property
    def __getitem__(self):
        return lambda k: k * 2


t("property __getitem__", lambda: PropGetItem()[21])


class PropSetItem:
    @property
    def __setitem__(self):
        return lambda k, v: print(f"  [setitem {k}={v}]")


def _setitem():
    o = PropSetItem()
    o[1] = 2
    return "done"


t("property __setitem__", _setitem)


class PropCall:
    @property
    def __call__(self):
        return lambda *a: ("called", a)


t("property __call__", lambda: PropCall()(1, 2))


class PropCtx:
    @property
    def __enter__(self):
        return lambda: "ENTERED"

    @property
    def __exit__(self):
        return lambda *a: False


def _ctx():
    with PropCtx() as v:
        return v


t("property __enter__/__exit__", _ctx)


class PropAdd:
    @property
    def __add__(self):
        return lambda other: ("added", other)


t("property __add__", lambda: PropAdd() + 10)


class PropEq:
    @property
    def __eq__(self):
        return lambda other: "EQ!"


t("property __eq__", lambda: PropEq() == 1)


class PropIndex:
    @property
    def __index__(self):
        return lambda: 3


t("property __index__", lambda: [0, 1, 2, 3, 4][PropIndex()])


# --- user-defined descriptors ---------------------------------------------
class NonData:
    def __get__(self, obj, objtype=None):
        print(f"  [NonData.__get__ obj={type(obj).__name__} objtype={objtype.__name__}]")
        return lambda: 7


class UsesNonData:
    __len__ = NonData()


t("user non-data descriptor", lambda: len(UsesNonData()))


class DataDesc:
    def __get__(self, obj, objtype=None):
        return lambda: 8

    def __set__(self, obj, val):
        print(f"  [DataDesc.__set__ {val}]")


class UsesData:
    __len__ = DataDesc()


t("user data descriptor", lambda: len(UsesData()))


def _data_desc_set():
    o = UsesData()
    o.__len__ = 5
    return len(o)


t("data descriptor __set__ then len", _data_desc_set)


class DescValue:
    def __get__(self, obj, objtype=None):
        print("  [DescValue.__get__ ran]")
        return 5


class UsesDescValue:
    __len__ = DescValue()


t("user descriptor -> non-callable", lambda: len(UsesDescValue()))


# A slot object that is both callable and a descriptor: binding wins, exactly
# as in CPython -- `tp_descr_get` is consulted without regard to callability.
class BothCallableAndDesc:
    def __call__(self, *a):
        return ("CALLED-DIRECTLY", a)

    def __get__(self, obj, objtype=None):
        return lambda *a: ("BOUND-VIA-GET", a)


class UsesBoth:
    __add__ = BothCallableAndDesc()


t("callable descriptor binds, not calls", lambda: UsesBoth() + 3)


# --- exceptions raised inside __get__ propagate ---------------------------
class RaisesAttr:
    def __get__(self, obj, objtype=None):
        raise AttributeError("boom-attr")


class UsesRaisesAttrLen:
    __len__ = RaisesAttr()


t("__get__ raises AttributeError", lambda: len(UsesRaisesAttrLen()))


class UsesRaisesAttrStr:
    __str__ = RaisesAttr()


t("__get__ raises AttributeError (str)", lambda: str(UsesRaisesAttrStr()))


class RaisesVal:
    def __get__(self, obj, objtype=None):
        raise ValueError("boom-val")


class UsesRaisesVal:
    __len__ = RaisesVal()


t("__get__ raises ValueError", lambda: len(UsesRaisesVal()))


class PropRaises:
    @property
    def __len__(self):
        raise AttributeError("prop-boom")


t("property getter raises", lambda: len(PropRaises()))


class NoGetter:
    __len__ = property()


t("property with no getter", lambda: len(NoGetter()))


# --- functools.cached_property --------------------------------------------
class CProp:
    @functools.cached_property
    def __len__(self):
        print("  [cached_property accessor ran]")
        return lambda: 11


_cp = CProp()
# The accessor runs once per instance: `cached_property.__get__` consults the
# instance dict itself, so a slot lookup (which never reads instance storage)
# still sees the cache on the second call.
t("cached_property first", lambda: len(_cp))
t("cached_property second", lambda: len(_cp))


# --- inheritance and metaclasses ------------------------------------------
class BaseDesc:
    __len__ = NonData()


class SubDesc(BaseDesc):
    pass


t("inherited descriptor", lambda: len(SubDesc()))


class BaseProp:
    @property
    def __len__(self):
        return lambda: 12


class SubProp(BaseProp):
    pass


t("inherited property", lambda: len(SubProp()))


class SubOverride(BaseProp):
    def __len__(self):
        return 200


t("subclass overrides property dunder", lambda: len(SubOverride()))


class MetaLen(type):
    @property
    def __len__(cls):
        print(f"  [metaclass getter cls={cls.__name__}]")
        return lambda: 13


class WithMetaLen(metaclass=MetaLen):
    pass


t("metaclass property __len__", lambda: len(WithMetaLen))


class MetaCall(type):
    @property
    def __call__(cls):
        return lambda *a: ("meta-called", a)


class WithMetaCall(metaclass=MetaCall):
    pass


t("metaclass property __call__", lambda: WithMetaCall(1))


# --- construction slots ---------------------------------------------------
class PropInit:
    @property
    def __init__(self):
        def init(*a):
            print(f"  [property __init__ args={a}]")

        return init


t("property __init__", lambda: type(PropInit(7)).__name__)


class PropInitBad:
    @property
    def __init__(self):
        return 5


t("property __init__ -> non-callable", lambda: PropInitBad())


class DescInit:
    class D:
        def __get__(self, obj, objtype=None):
            return lambda *a: print(f"  [desc __init__ args={a}]")

    __init__ = D()


t("user descriptor __init__", lambda: type(DescInit(3)).__name__)


class IntInit:
    __init__ = 5


t("non-callable __init__", lambda: IntInit())


# --- `__get__` is NOT itself descriptor-bound (raw lookup, 3-arg call) -----
class InnerGet:
    def __get__(self, obj, objtype=None):
        return lambda o, ot=None: (lambda: 14)


class OuterDesc:
    # `slot_tp_descr_get` finds this with a raw `_PyType_Lookup` and calls it
    # directly, so the InnerGet instance is never bound -- and, not being
    # callable, reports itself rather than `OuterDesc`.
    __get__ = InnerGet()


class UsesOuter:
    __len__ = OuterDesc()


t("__get__ that is a descriptor", lambda: len(UsesOuter()))


class CallableGet:
    def __call__(self, descr, obj, objtype=None):
        print(f"  [CallableGet.__call__ descr={type(descr).__name__}]")
        return lambda: 42


class OuterCallableGet:
    __get__ = CallableGet()


class UsesCallableGet:
    __len__ = OuterCallableGet()


t("callable-instance __get__ gets 3 args", lambda: len(UsesCallableGet()))


class GetIsProp:
    @property
    def __get__(self):
        return lambda obj, objtype=None: "BOUND"


class HostGetIsProp:
    attr = GetIsProp()


t("__get__ as a property (attribute access)", lambda: HostGetIsProp().attr)


# A class whose `__get__` is an instance of itself: the raw-lookup rule means
# this terminates instead of recursing forever.
class SelfLoop:
    pass


SelfLoop.__get__ = SelfLoop()


class UsesSelfLoop:
    __len__ = SelfLoop()


t("self-referential __get__ terminates", lambda: len(UsesSelfLoop()))


# --- `__set_name__` still fires for a descriptor in a dunder slot ----------
class SetNameDesc:
    def __set_name__(self, owner, name):
        print(f"  [__set_name__ owner={owner.__name__} name={name}]")
        self.name = name

    def __get__(self, obj, objtype=None):
        return lambda: len(self.name)


class UsesSetName:
    __len__ = SetNameDesc()


t("__set_name__ descriptor", lambda: len(UsesSetName()))


# --- rebinding a dunder mid-program invalidates any cached slot -----------
class Rebind:
    def __len__(self):
        return 1


_r = Rebind()
t("rebind: plain def", lambda: len(_r))
Rebind.__len__ = property(lambda self: (lambda: 99))
t("rebind: -> property", lambda: len(_r))
Rebind.__len__ = NonData()
t("rebind: -> descriptor", lambda: len(_r))
Rebind.__len__ = lambda self: 3
t("rebind: -> plain def", lambda: len(_r))


# The same rebind against a *warm* dispatch site, so a cached slot that was
# specialised to a plain function must be invalidated when it becomes a
# property (and back again).
class Warm:
    def __len__(self):
        return 1


_w = Warm()
_seen = []
for _i in range(2000):
    _seen.append(len(_w))
    if _i == 1000:
        Warm.__len__ = property(lambda self: (lambda: 2))
t("warm rebind def->property", lambda: (_seen[0], _seen[1000], _seen[1001], _seen[-1]))


class Warm2:
    __len__ = property(lambda self: (lambda: 5))


_w2 = Warm2()
_seen2 = []
for _i in range(2000):
    _seen2.append(len(_w2))
    if _i == 1000:
        Warm2.__len__ = lambda self: 6
t("warm rebind property->def", lambda: (_seen2[0], _seen2[1000], _seen2[1001], _seen2[-1]))


# --- PR #2943 / #2959 behaviour is preserved ------------------------------
class KeepStatic:
    __len__ = staticmethod(lambda: 31)


t("staticmethod slot", lambda: len(KeepStatic()))


class KeepClass:
    __len__ = classmethod(lambda cls: 32)


t("classmethod slot", lambda: len(KeepClass()))


class KeepPartial:
    __len__ = functools.partial(lambda x: 33, 0)


t("functools.partial slot", lambda: len(KeepPartial()))


class KeepCallable:
    class C:
        def __call__(self):
            return 34

    __len__ = C()


t("callable instance slot", lambda: len(KeepCallable()))


class KeepBuiltin:
    # A bare builtin is not a descriptor, so it is called receiverless.
    __hash__ = id


t("builtin slot stays receiverless", lambda: hash(KeepBuiltin()))


class KeepBad:
    __len__ = 5


t("non-callable slot", lambda: len(KeepBad()))
