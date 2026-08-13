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


def iter_consumers(label, factory):
    t(f"{label} list", lambda: list(factory()))
    t(f"{label} iter", lambda: next(iter(factory())))
    t(f"{label} for", lambda: [item for item in factory()])
def iter_error(fn):
    try:
        fn()
    except Exception as e:
        return type(e).__name__, str(e)
def iter_consumer_errors(label, factory):
    consumers = (list, iter, lambda x: [v for v in x])
    errors = (iter_error(lambda c=c: c(factory())) for c in consumers)
    print(f"{label}: {[(k, m.endswith('bad argument to internal function')) for k, m in errors]}")
class IterBindRaises:
    def __get__(self, obj, objtype=None):
        raise RuntimeError("iter-bind")

IterFallback = type("IterFallback", (), {"__iter__": IterBindRaises(), "__getitem__": lambda self, index: (10, 11)[index]})

class IterNoFallback: __iter__ = IterBindRaises()

iter_consumers("iter bind error fallback", IterFallback)
iter_consumers("iter bind error no fallback", IterNoFallback)

class IterBodyRaises:
    class Slot:
        def __get__(self, obj, objtype=None):
            def fail():
                raise RuntimeError("iter-body")

            return fail

    __iter__ = Slot()

class NoneDescriptor:
    def __get__(self, obj, objtype=None):
        return None

class IterBindsNone:
    __iter__ = NoneDescriptor()

    def __getitem__(self, index):
        return index

t("iter callable body error", lambda: list(IterBodyRaises()))
t("iter descriptor returns None", lambda: list(IterBindsNone()))
t("iter bind membership hit", lambda: 10 in IterFallback())
t("iter bind membership miss", lambda: 99 in IterFallback())
t("iter bind membership no fallback", lambda: 1 in IterNoFallback())
t("iter body membership error", lambda: 1 in IterBodyRaises())
t("iter None membership", lambda: (iter_error(lambda: 1 in type("RawNone", (), {"__iter__": None})()), iter_error(lambda: 1 in IterBindsNone())))
IterStaticNone = type("IterStaticNone", (), {"__iter__": staticmethod(None)})
IterClassNone = type("IterClassNone", (), {"__iter__": classmethod(None)})
IterTypeRaises = type("IterTypeRaises", (), {"__iter__": lambda self: (_ for _ in ()).throw(TypeError("iter-type"))})
IterInvalid = type("IterInvalid", (), {"__iter__": lambda self: []})
iter_consumers("iter static None", IterStaticNone)
iter_consumers("iter class None", IterClassNone)
t("iter wrapper None membership", lambda: (iter_error(lambda: 1 in IterStaticNone()), iter_error(lambda: 1 in IterClassNone())))
t("iter TypeError membership", lambda: (iter_error(lambda: 1 in IterTypeRaises()), iter_error(lambda: 1 in IterInvalid())))
class IterListBacking(list): __iter__ = IterBindRaises()
IterListGetitem = type("IterListGetitem", (list,), {"__iter__": IterBindRaises(), "__getitem__": lambda self, index: (90, 91)[index]})
class IterSetBacking(set): __iter__ = IterBindRaises()
class IterDictGetitem(dict):
    __iter__ = IterBindRaises()
    def __getitem__(self, index):
        return index + 70
IterNativeDescriptor = type("IterNativeDescriptor", (), {"__iter__": dict.__dict__["fromkeys"], "__getitem__": lambda self, index: (60, 61)[index]})
MemberSlot = type("MemberSlot", (), {"__slots__": ("value",)}).__dict__["value"]
IterMemberDescriptor = type("IterMemberDescriptor", (), {"__iter__": MemberSlot, "__getitem__": lambda self, index: (80, 81)[index]})
IterGetsetDescriptor = type("IterGetsetDescriptor", (), {"__iter__": int.real, "__getitem__": lambda self, index: (70, 71)[index]})
IterViewGetset = type("IterViewGetset", (), {"__iter__": type({}.keys()).mapping, "__getitem__": lambda self, index: (50, 51)[index]})
UnboundSuper = super(type("SuperOwner", (), {})); IterSuperDescriptor = type("IterSuperDescriptor", (), {"__iter__": UnboundSuper, "__getitem__": lambda self, index: (20, 21)[index]})
iter_consumers("iter bind list backing", lambda: IterListBacking([1, 2]))
iter_consumers("iter bind list getitem", lambda: IterListGetitem([1, 2]))
iter_consumers("iter bind set", lambda: IterSetBacking([1, 2]))
iter_consumer_errors("iter bind dict getitem", IterDictGetitem)
iter_consumers("iter native descriptor fallback", IterNativeDescriptor)
t("iter native descriptor membership", lambda: (60 in IterNativeDescriptor(), 99 in IterNativeDescriptor()))
iter_consumers("iter member descriptor fallback", IterMemberDescriptor)
t("iter member descriptor membership", lambda: (80 in IterMemberDescriptor(), 99 in IterMemberDescriptor()))
iter_consumers("iter getset fallback", IterGetsetDescriptor)
iter_consumers("iter view getset fallback", IterViewGetset)
iter_consumers("iter super fallback", IterSuperDescriptor); t("iter super membership", lambda: (20 in IterSuperDescriptor(), 99 in IterSuperDescriptor()))
IntGetsetIter = type("IntGetsetIter", (int,), {"__iter__": int.real}); t("valid getset iter", lambda: (iter_error(lambda: list(IntGetsetIter(5))), iter_error(lambda: 1 in IntGetsetIter(5))))
class PropHash:
    @property
    def __hash__(self):
        return lambda: 99


t("property __hash__", lambda: hash(PropHash()))


def hash_consumers(label, factory):
    t(f"{label} hash", lambda: hash(factory()))
    t(f"{label} dict", lambda: len({factory(): 1}))
    t(f"{label} set", lambda: len({factory()}))


class HashBindRaises:
    def __get__(self, obj, objtype=None):
        raise RuntimeError("hash-bind")

class HashBindError: __hash__ = HashBindRaises()

class HashBindsNone: __hash__ = NoneDescriptor()

class HashBodyRaises:
    class Slot:
        def __get__(self, obj, objtype=None):
            def fail():
                raise RuntimeError("hash-body")

            return fail

    __hash__ = Slot()

hash_consumers("hash bind error", HashBindError)
hash_consumers("hash descriptor returns None", HashBindsNone)
hash_consumers("hash callable body error", HashBodyRaises)
HashStaticNone = type("HashStaticNone", (), {"__hash__": staticmethod(None)})
HashClassNone = type("HashClassNone", (), {"__hash__": classmethod(None)})
hash_consumers("hash static None", HashStaticNone)
hash_consumers("hash class None", HashClassNone)
HashNativeDescriptor = type("HashNativeDescriptor", (), {"__hash__": dict.__dict__["fromkeys"]}); hash_consumers("hash native descriptor", HashNativeDescriptor)
HashMemberDescriptor = type("HashMemberDescriptor", (), {"__hash__": MemberSlot})
hash_consumers("hash member descriptor", HashMemberDescriptor)
HashGetsetDescriptor = type("HashGetsetDescriptor", (), {"__hash__": int.real}); HashViewGetset = type("HashViewGetset", (), {"__hash__": type({}.keys()).mapping})
HashSuperDescriptor = type("HashSuperDescriptor", (), {"__hash__": UnboundSuper}); hash_consumers("hash super descriptor", HashSuperDescriptor)
hash_consumers("hash getset descriptor", HashGetsetDescriptor); hash_consumers("hash view getset", HashViewGetset)
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


class MetaIterGetitem(type):
    __iter__ = IterBindRaises()

    def __getitem__(cls, index):
        if index < 2:
            return index + 30
        raise IndexError


class MetaIterNoGetitem(type): __iter__ = IterBindRaises()

class MetaIterBody(type): __iter__ = IterBodyRaises.Slot()

class MetaIterNone(type): __iter__ = NoneDescriptor()

class MetaIterFallback(metaclass=MetaIterGetitem): pass
class MetaIterMissing(metaclass=MetaIterNoGetitem): pass
class MetaIterRaises(metaclass=MetaIterBody): pass
class MetaIterNoniterable(metaclass=MetaIterNone): pass
class MetaIterType(type): __iter__ = lambda cls: (print("  [meta type body]"), (_ for _ in ()).throw(TypeError("meta-type")))[1]
class MetaIterInvalid(type): __iter__ = lambda cls: (print("  [meta invalid body]"), [])[1]
class MetaGetOnly(type): __getitem__ = lambda cls, index: (40, 41)[index]
class MetaTypeRaises(metaclass=MetaIterType): pass
class MetaInvalid(metaclass=MetaIterInvalid): pass
class MetaOnlyGetitem(metaclass=MetaGetOnly): pass
MetaNativeIter = type("MetaNativeIter", (type,), {"__iter__": dict.__dict__["fromkeys"], "__getitem__": lambda cls, index: (4, 5)[index]}); MetaNative = MetaNativeIter("MetaNative", (), {})
iter_consumers("metaclass iter bind fallback", lambda: MetaIterFallback)
iter_consumers("metaclass iter bind no fallback", lambda: MetaIterMissing)
iter_consumers("metaclass getitem only", lambda: MetaOnlyGetitem)
t("metaclass iter body error", lambda: list(MetaIterRaises))
t("metaclass iter returns None", lambda: list(MetaIterNoniterable))
t("metaclass iter membership", lambda: (30 in MetaIterFallback, 99 in MetaIterFallback, 40 in MetaOnlyGetitem, iter_error(lambda: 1 in MetaIterMissing), iter_error(lambda: 1 in MetaIterNoniterable), iter_error(lambda: 1 in MetaIterRaises)))
t("metaclass TypeError membership", lambda: (iter_error(lambda: 1 in MetaTypeRaises), iter_error(lambda: 1 in MetaInvalid)))
iter_consumers("metaclass native descriptor", lambda: MetaNative); t("metaclass native membership", lambda: (4 in MetaNative, 99 in MetaNative))


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


class NewDescriptor:
    def __get__(self, obj, owner=None):
        print(f"  [new descriptor obj_is_none={obj is None} owner={owner.__name__}]")

        def allocate(cls, token):
            result = object.__new__(cls)
            result.new_seen = (cls.__name__, token)
            return result

        return allocate


class UsesNewDescriptor:
    __new__ = NewDescriptor()
    def __init__(self, token):
        self.init_seen = token
def use_new_descriptor():
    result = UsesNewDescriptor("ok")
    return result.new_seen, result.init_seen


t("descriptor __new__ class bind", use_new_descriptor)


class NewBindRaises:
    def __get__(self, obj, owner=None):
        raise RuntimeError("new-bind")


class UsesNewBindRaises: __new__ = NewBindRaises()


class NewBindsInt:
    def __get__(self, obj, owner=None):
        return 5


class UsesNewBindsInt: __new__ = NewBindsInt()


class NewIsInt: __new__ = 5


t("descriptor __new__ bind error", lambda: UsesNewBindRaises()); t("descriptor __new__ non-callable", lambda: UsesNewBindsInt())
t("raw __new__ non-callable", lambda: NewIsInt())
NewCallable = type("NewCallable", (), {"__call__": lambda self, *args: tuple(x.__name__ if isinstance(x, type) else x for x in args)})
NewStatic = type("NewStatic", (), {"__new__": staticmethod(NewCallable())})
NewClass = type("NewClass", (), {"__new__": classmethod(NewCallable())})
NewStaticNone = type("NewStaticNone", (), {"__new__": staticmethod(None)})
NewStaticInt = type("NewStaticInt", (), {"__new__": staticmethod(5)})
t("staticmethod callable __new__", lambda: NewStatic("x"))
t("classmethod callable __new__", lambda: NewClass("x"))
t("staticmethod None __new__", lambda: NewStaticNone())
t("staticmethod int __new__", lambda: NewStaticInt())
PostInitDescriptor = type("PostInitDescriptor", (), {"__get__": lambda self, obj, owner=None: lambda token: setattr(obj, "init_seen", ("descriptor", token))})(); PostInitCallable = type("PostInitCallable", (), {"__call__": lambda self, token: print(f"  [post callable {token}]")})()
PostNewDesc = type("PostNewDesc", (), {"__new__": NewDescriptor(), "__init__": PostInitDescriptor}); PostNewCallable = type("PostNewCallable", (), {"__new__": NewDescriptor(), "__init__": PostInitCallable}); PostNewBad = type("PostNewBad", (), {"__new__": NewDescriptor(), "__init__": 5})
t("post descriptor new init", lambda: (PostNewDesc("x").init_seen, type(PostNewCallable("y")).__name__, iter_error(lambda: PostNewBad("z"))))
FunctionNew = lambda cls, token: object.__new__(cls); FunctionNewDesc = type("FunctionNewDesc", (), {"__new__": FunctionNew, "__init__": PostInitDescriptor})
FunctionNewCallable = type("FunctionNewCallable", (), {"__new__": FunctionNew, "__init__": PostInitCallable}); FunctionNewBad = type("FunctionNewBad", (), {"__new__": FunctionNew, "__init__": 5})
t("post function new init", lambda: (FunctionNewDesc("a").init_seen, type(FunctionNewCallable("b")).__name__, iter_error(lambda: FunctionNewBad("c"))))
InitToken = lambda self, token: setattr(self, "init_seen", token); RawObjectNew = type("RawObjectNew", (), {"__new__": object.__new__, "__init__": InitToken}); StaticObjectNew = type("StaticObjectNew", (), {"__new__": staticmethod(object.__new__), "__init__": InitToken}); ClassObjectNew = type("ClassObjectNew", (), {"__new__": classmethod(object.__new__), "__init__": InitToken})
t("wrapped object new provenance", lambda: (RawObjectNew("raw").init_seen, iter_error(lambda: StaticObjectNew("static")), iter_error(lambda: ClassObjectNew("class"))))

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


# --- in-place operators obey the same descriptor rule ---------------------
# The augmented-assignment slot is dispatched by its own site, which used to
# accept only plain functions and answer "not defined" for everything else --
# silently falling through to `__add__` / the backing fallback instead of
# binding the descriptor.
class IDesc:
    def __get__(self, obj, objtype=None):
        print(f"  [IDesc.__get__ objtype={objtype.__name__}]")
        return lambda other: ("idesc", other)


def _augmented(cls, op):
    def go():
        ns = {"o": cls()}
        exec(f"o {op} 5", ns)
        return ns["o"]

    return go


for _op, _name in [
    ("+=", "__iadd__"),
    ("-=", "__isub__"),
    ("*=", "__imul__"),
    ("//=", "__ifloordiv__"),
    ("/=", "__itruediv__"),
    ("%=", "__imod__"),
    ("**=", "__ipow__"),
    ("<<=", "__ilshift__"),
    (">>=", "__irshift__"),
    ("&=", "__iand__"),
    ("|=", "__ior__"),
    ("^=", "__ixor__"),
    ("@=", "__imatmul__"),
]:
    _C = type("I_" + _name.strip("_"), (), {_name: IDesc()})
    t(f"descriptor {_name}", _augmented(_C, _op))


class PropIadd:
    @property
    def __iadd__(self):
        print("  [PropIadd getter ran]")
        return lambda other: ("prop-iadd", other)


t("property __iadd__", _augmented(PropIadd, "+="))


class PropIaddValue:
    @property
    def __iadd__(self):
        print("  [PropIaddValue getter ran]")
        return 7


# The getter runs; the error names its result, exactly as for `__add__`.
t("property __iadd__ -> non-callable", _augmented(PropIaddValue, "+="))


class IaddNonCallable:
    __iadd__ = 5

    def __add__(self, other):
        return ("add-fallback", other)


# A non-callable in-place slot is an error, not a reason to fall back to
# `__add__` (issue #2055).
t("non-callable __iadd__ shadows __add__", _augmented(IaddNonCallable, "+="))


class IaddCallableInstance:
    class C:
        def __call__(self, other):
            return ("callable-instance", other)

    __iadd__ = C()


t("callable-instance __iadd__", _augmented(IaddCallableInstance, "+="))


class IaddNotImplemented:
    class D:
        def __get__(self, obj, objtype=None):
            return lambda other: NotImplemented

    __iadd__ = D()

    def __add__(self, other):
        return ("add-fallback", other)


# A bound descriptor returning NotImplemented still falls back to `__add__`.
t("descriptor __iadd__ -> NotImplemented", _augmented(IaddNotImplemented, "+="))


class IaddRaises:
    class R:
        def __get__(self, obj, objtype=None):
            raise ValueError("iadd-get-boom")

    __iadd__ = R()

    def __add__(self, other):
        return ("add-fallback", other)


# The getter's exception propagates; it is not swallowed into the fallback.
t("__iadd__ __get__ raises", _augmented(IaddRaises, "+="))


class IaddStatic:
    __iadd__ = staticmethod(lambda other: ("static-iadd", other))


t("staticmethod __iadd__", _augmented(IaddStatic, "+="))
