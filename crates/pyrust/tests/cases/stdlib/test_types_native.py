"""CPython 3.12.3 parity for the native `types` module (issue #3460).

Covers the class-creation helpers, DynamicClassAttribute, coroutine /
_GeneratorWrapper, SimpleNamespace, MappingProxyType and the exported names.
"""
import enum
import sys
import types


def show(label, thunk):
    try:
        print(label, "->", repr(thunk()))
    except BaseException as exc:
        print(label, "raised", type(exc).__name__, exc)


# ── __all__ and the exported names ──────────────────────────────────────────
print(types.__all__)
print(len(types.__all__), len(set(types.__all__)))
print(all(hasattr(types, name) for name in types.__all__))
print(sorted(set(dir(types)) - set(types.__all__) - {
    "__builtins__", "__cached__", "__file__", "__loader__", "__name__",
    "__package__", "__spec__", "__doc__", "__all__",
}))
print(types.__doc__)
print(types.__name__)

for name in ("_mark_iterable_coroutine", "_is_iterable_coroutine",
             "_is_generator_wrapper_candidate", "_CoroutineWrapper", "sys", "_f", "exc"):
    print(name, hasattr(types, name))

print(types.LambdaType is types.FunctionType)
print(types.BuiltinMethodType is types.BuiltinFunctionType)
print(type(sys.modules[__name__]) is types.ModuleType)
print(types.NoneType is type(None), types.EllipsisType is type(...),
      types.NotImplementedType is type(NotImplemented))
print(types.GenericAlias is type(list[int]), types.UnionType is type(int | str))
print(types.MappingProxyType is type(type.__dict__))
print(types.GeneratorType is type(x for x in ()))
print(types.MethodType is type(types.SimpleNamespace().__repr__))
print(types.CodeType is type(show.__code__))
print(types.TracebackType is type(sys.exc_info()[2]))


# ── resolve_bases ───────────────────────────────────────────────────────────
class Plain:
    pass


class OtherPlain:
    pass


class Entry:
    """Object with a __mro_entries__ hook."""

    def __init__(self, *entries):
        self.entries = entries
        self.seen = []

    def __mro_entries__(self, bases):
        self.seen.append(bases)
        return self.entries


plain_bases = (Plain, OtherPlain)
print(types.resolve_bases(plain_bases) is plain_bases)
print(types.resolve_bases(()) == ())
empty = ()
print(types.resolve_bases(empty) is empty)

entry = Entry(Plain, OtherPlain)
mixed = (int, entry, float)
resolved = types.resolve_bases(mixed)
print(resolved == (int, Plain, OtherPlain, float), type(resolved).__name__)
print(entry.seen == [mixed], entry.seen[0] is mixed)

shrink = Entry()
print(types.resolve_bases((shrink, Plain)) == (Plain,))
print(types.resolve_bases((Entry(), Entry(), Plain)) == (Plain,))

two = Entry(Plain, OtherPlain)
three = Entry(int, str, bytes)
print(types.resolve_bases((two, three, object)) == (Plain, OtherPlain, int, str, bytes, object))
print(types.resolve_bases([Entry(Plain), int]) == (Plain, int))


class NotTuple:
    def __mro_entries__(self, bases):
        return [Plain]


show("mro_entries non-tuple", lambda: types.resolve_bases((NotTuple(),)))


class TupleSub(tuple):
    pass


class SubTupleEntry:
    def __mro_entries__(self, bases):
        return TupleSub((Plain,))


print(types.resolve_bases((SubTupleEntry(),)) == (Plain,))


class Raising:
    def __mro_entries__(self, bases):
        raise ValueError("boom")


show("mro_entries raises", lambda: types.resolve_bases((Raising(),)))
show("resolve_bases non-iterable", lambda: types.resolve_bases(5))
show("resolve_bases no args", lambda: types.resolve_bases())
show("resolve_bases two args", lambda: types.resolve_bases((), ()))
show("resolve_bases keyword", lambda: types.resolve_bases(bases=(Plain,)))
show("resolve_bases bad keyword", lambda: types.resolve_bases(base=(Plain,)))


class Aux:
    pass


class NoEntries:
    pass


# Non-class objects without __mro_entries__ are left in place.
odd = (NoEntries(),)
print(types.resolve_bases(odd) is odd)

# Metaclass instances (classes) never get __mro_entries__ consulted.
class HasEntriesClass:
    def __mro_entries__(self, bases):
        raise AssertionError("must not be called for classes")


print(types.resolve_bases((HasEntriesClass,)) == (HasEntriesClass,))


# ── _calculate_meta ─────────────────────────────────────────────────────────
class MetaA(type):
    pass


class MetaB(MetaA):
    pass


class MetaC(type):
    pass


class WithA(metaclass=MetaA):
    pass


class WithB(metaclass=MetaB):
    pass


class WithC(metaclass=MetaC):
    pass


print(types._calculate_meta(type, ()) is type)
print(types._calculate_meta(type, (Plain,)) is type)
print(types._calculate_meta(type, (WithA,)) is MetaA)
print(types._calculate_meta(MetaA, (WithB,)) is MetaB)
print(types._calculate_meta(MetaB, (WithA,)) is MetaB)
print(types._calculate_meta(MetaA, (WithA, WithB, Plain)) is MetaB)
show("meta conflict", lambda: types._calculate_meta(MetaA, (WithC,)))
show("meta conflict order", lambda: types._calculate_meta(type, (WithB, WithC)))
show("calculate_meta args", lambda: types._calculate_meta(type))

# ── prepare_class ───────────────────────────────────────────────────────────
meta, ns, kw = types.prepare_class("N")
print(meta is type, ns, kw)
meta, ns, kw = types.prepare_class("N", (Plain,))
print(meta is type, ns, kw)
meta, ns, kw = types.prepare_class("N", (WithB,))
print(meta is MetaB, ns, kw)
meta, ns, kw = types.prepare_class("N", (Plain,), {"metaclass": MetaA})
print(meta is MetaA, ns, kw)

caller_kwds = {"metaclass": MetaA, "extra": 1}
meta, ns, kw = types.prepare_class("N", (WithB,), caller_kwds)
print(meta is MetaB, kw, caller_kwds)
print(kw is caller_kwds)

only_kw = {"x": 1}
meta, ns, kw = types.prepare_class("N", (), only_kw)
print(kw == only_kw, kw is only_kw)
none_meta, none_ns, none_kw = types.prepare_class("N", (), None)
print(none_meta is type, none_ns, none_kw)


class PrepMeta(type):
    prepared = []

    @classmethod
    def __prepare__(mcs, name, bases, **kwds):
        mcs.prepared.append((mcs.__name__, name, bases, dict(kwds)))
        return {"prepared_by": mcs.__name__, **{f"kw_{k}": v for k, v in kwds.items()}}


class PrepChild(PrepMeta):
    pass


meta, ns, kw = types.prepare_class("Prep", (Plain,), {"metaclass": PrepMeta, "a": 1, "b": 2})
print(meta is PrepMeta, ns, kw)
print(PrepMeta.prepared)
PrepMeta.prepared.clear()


class UsesPrep(metaclass=PrepChild):
    pass


meta, ns, kw = types.prepare_class("Prep2", (UsesPrep,), {"z": 0})
print(meta is PrepChild, ns, kw)
print(PrepMeta.prepared)
PrepMeta.prepared.clear()

# `metaclass` that is not a type: used as-is, no _calculate_meta.
def factory_meta(name, bases, ns, **kwds):
    return ("factory", name, bases, sorted(ns), kwds)


meta, ns, kw = types.prepare_class("F", (Plain,), {"metaclass": factory_meta})
print(meta is factory_meta, ns, kw)


class PrepareReturnsOdd(type):
    @classmethod
    def __prepare__(mcs, name, bases):
        return "not a mapping"


meta, ns, kw = types.prepare_class("Odd", (), {"metaclass": PrepareReturnsOdd})
print(meta is PrepareReturnsOdd, ns, kw)
show("prepare unexpected kw", lambda: types.prepare_class("Odd", (), {"metaclass": PrepareReturnsOdd, "k": 1}))
show("prepare conflict", lambda: types.prepare_class("C", (WithB, WithC)))
show("prepare conflict via metaclass kw", lambda: types.prepare_class("C", (WithC,), {"metaclass": MetaA}))
show("prepare no args", lambda: types.prepare_class())
show("prepare too many", lambda: types.prepare_class("a", (), None, 4))
show("prepare bad keyword", lambda: types.prepare_class("a", bad=1))
show("prepare name keyword", lambda: types.prepare_class(name="a", bases=(Plain,), kwds={"q": 1}))
show("prepare non-str kwds key", lambda: types.prepare_class("a", (), {1: 2, "metaclass": PrepMeta}))
show("prepare non-mapping kwds", lambda: types.prepare_class("a", (), 5))


# ── new_class ───────────────────────────────────────────────────────────────
Simple = types.new_class("Simple")
print(Simple.__name__, Simple.__bases__, type(Simple) is type, Simple.__qualname__)

WithBase = types.new_class("WithBase", (Plain,))
print(WithBase.__bases__ == (Plain,), issubclass(WithBase, Plain))
print("__orig_bases__" in WithBase.__dict__)


def body(ns):
    ns["x"] = 1
    ns["method"] = lambda self: ("method", self.x)
    ns["__qualname__"] = "Custom.Qual"


Body = types.new_class("Body", (Plain,), None, body)
print(Body.x, Body().method(), Body.__qualname__)
Body2 = types.new_class("Body2", (), exec_body=body)
print(Body2.x, Body2.__bases__)
Body3 = types.new_class("Body3", exec_body=lambda ns: ns.update(y=2))
print(Body3.y)

orig_entry = Entry(Plain, OtherPlain)
Generic1 = types.new_class("Generic1", (orig_entry,))
print(Generic1.__bases__ == (Plain, OtherPlain))
print(Generic1.__dict__["__orig_bases__"] == (orig_entry,))
print(Generic1.__dict__["__orig_bases__"][0] is orig_entry)
print(orig_entry.seen == [(orig_entry,)])

# __orig_bases__ is only recorded when resolve_bases changed the bases.
Generic2 = types.new_class("Generic2", (Aux, orig_entry))
print(Generic2.__dict__["__orig_bases__"] == (Aux, orig_entry))

# exec_body receives the namespace after __prepare__ and before __orig_bases__.
seen_ns = []


def watch(ns):
    seen_ns.append(sorted(ns))


types.new_class("Watch", (Entry(Plain),), None, watch)
print(seen_ns)


class HookedOK:
    received = []

    def __init_subclass__(cls, flag=None, n=None, **kwds):
        HookedOK.received.append((cls.__name__, flag, n, kwds))


types.new_class("ChildOK", (HookedOK,), {"flag": True, "n": 3})
print(HookedOK.received)

kwds_in = {"flag": 1, "n": 2}
types.new_class("ChildOK2", (HookedOK,), kwds_in)
print(kwds_in)

# metaclass keyword selects the metaclass; the keyword is not forwarded.
Metaed = types.new_class("Metaed", (), {"metaclass": MetaA})
print(type(Metaed) is MetaA, Metaed.__bases__)
Derived = types.new_class("Derived", (WithB,), {"metaclass": MetaA})
print(type(Derived) is MetaB)
show("new_class conflict", lambda: types.new_class("Bad", (WithB, WithC)))

# __prepare__ drives the namespace.
class OrderedNS(dict):
    pass


class OrderedMeta(type):
    @classmethod
    def __prepare__(mcs, name, bases, **kwds):
        return OrderedNS(from_prepare=True)

    def __new__(mcs, name, bases, ns, **kwds):
        cls = super().__new__(mcs, name, bases, dict(ns))
        cls.ns_type = type(ns).__name__
        cls.meta_kwds = kwds
        return cls

    def __init__(cls, name, bases, ns, **kwds):
        super().__init__(name, bases, ns)


Ordered = types.new_class("Ordered", (), {"metaclass": OrderedMeta, "opt": 5},
                          lambda ns: ns.__setitem__("added", 1))
print(Ordered.from_prepare, Ordered.added, Ordered.ns_type, Ordered.meta_kwds)

# Non-type metaclass (callable) receives (name, bases, ns, **kwds).
print(types.new_class("Fn", (Plain,), {"metaclass": factory_meta, "k": 1},
                      lambda ns: ns.update(a=1)))

# Metaclass picked from the bases when no keyword is given.
print(type(types.new_class("Inherit", (WithB,))) is MetaB)

# resolve_bases / prepare_class are looked up on the module at call time.
real_resolve, real_prepare = types.resolve_bases, types.prepare_class
calls = []


def spy_resolve(bases):
    calls.append("resolve")
    return real_resolve(bases)


def spy_prepare(name, bases=(), kwds=None):
    calls.append("prepare")
    return real_prepare(name, bases, kwds)


types.resolve_bases, types.prepare_class = spy_resolve, spy_prepare
try:
    types.new_class("Spied")
finally:
    types.resolve_bases, types.prepare_class = real_resolve, real_prepare
print(calls)

real_calc = types._calculate_meta
calc_calls = []


def spy_calc(meta, bases):
    calc_calls.append((meta.__name__, len(bases)))
    return real_calc(meta, bases)


types._calculate_meta = spy_calc
try:
    types.prepare_class("Spied", (Plain,))
    types.prepare_class("Spied", (), {"metaclass": factory_meta})
finally:
    types._calculate_meta = real_calc
print(calc_calls)

show("new_class no args", lambda: types.new_class())
show("new_class bad keyword", lambda: types.new_class("a", nope=1))
show("new_class too many", lambda: types.new_class("a", (), None, None, 5))
show("new_class name keyword", lambda: types.new_class(name="KW", bases=(Plain,)).__name__)
show("new_class exec_body non-callable", lambda: types.new_class("a", (), None, 5))
show("new_class name not str", lambda: types.new_class(5))
show("new_class bad base", lambda: types.new_class("a", (5,)))
show("new_class kwds bad key", lambda: types.new_class("a", (), {1: 2}))
show("new_class kwds unexpected", lambda: types.new_class("a", (), {"unexpected": 1}))


class SlotsMeta(type):
    pass


Slotted = types.new_class("Slotted", (), {"metaclass": SlotsMeta},
                          lambda ns: ns.update(__slots__=("a", "b")))
print(Slotted.__slots__, hasattr(Slotted(), "__dict__"))

# Instances built from dynamically created classes behave normally.
Dyn = types.new_class("Dyn", (dict,), None, lambda ns: ns.update(extra=lambda self: len(self)))
dyn = Dyn(a=1, b=2)
print(dyn.extra(), isinstance(dyn, dict), type(dyn).__name__)


# ── get_original_bases ──────────────────────────────────────────────────────
class Base:
    pass


class Sub(Base, OtherPlain):
    pass


print(types.get_original_bases(Base) == (object,))
print(types.get_original_bases(Sub) == (Base, OtherPlain))
print(types.get_original_bases(int) == (object,))
print(types.get_original_bases(bool) == (int,))
print(types.get_original_bases(Generic1) == (orig_entry,))
print(types.get_original_bases(Generic2) == (Aux, orig_entry))
print(types.get_original_bases(object) == ())


class GenericAliasBase(list[int]):
    pass


print(types.get_original_bases(GenericAliasBase) == (list[int],))


class FakeOrig(Base):
    __orig_bases__ = ("sentinel",)


print(types.get_original_bases(FakeOrig))
# Only the class' own __dict__ counts, not an inherited __orig_bases__.


class InheritsFake(FakeOrig):
    pass


print(types.get_original_bases(InheritsFake) == (FakeOrig,))
show("original_bases int instance", lambda: types.get_original_bases(1))
show("original_bases None", lambda: types.get_original_bases(None))
show("original_bases instance", lambda: types.get_original_bases(Base()))
show("original_bases str", lambda: types.get_original_bases("abc"))
show("original_bases keyword", lambda: types.get_original_bases(cls=int))
show("original_bases no args", lambda: types.get_original_bases())
show("original_bases two args", lambda: types.get_original_bases(int, int))


class HasBasesNoDict:
    __bases__ = ("fake",)


show("original_bases object with __bases__ only", lambda: types.get_original_bases(HasBasesNoDict()))

from typing import Generic, TypeVar

T = TypeVar("T")


class GenericUser(Generic[T]):
    pass


class GenericChild(GenericUser[int], float):
    pass


print(types.get_original_bases(GenericChild) == (GenericUser[int], float))
print(GenericChild.__bases__ == (GenericUser, float))
print(types.get_original_bases(GenericUser) == (Generic[T],))


# ── DynamicClassAttribute ───────────────────────────────────────────────────
DCA = types.DynamicClassAttribute
print(DCA, DCA.__module__, DCA.__name__, DCA.__qualname__)
print(DCA.__doc__)


class Widget:
    def __init__(self):
        self._value = 5
        self.log = []

    @DCA
    def value(self):
        "the value"
        return self._value

    @value.setter
    def value(self, new):
        self.log.append(("set", new))
        self._value = new

    @value.deleter
    def value(self):
        self.log.append(("del",))
        del self._value

    @DCA
    def readonly(self):
        return "ro"

    def __getattr__(self, name):
        return ("instance getattr", name)

    @classmethod
    def class_level(cls):
        return cls.value


w = Widget()
print(w.value)
w.value = 7
print(w.value, w.log)
del w.value
print(w.log, hasattr(w, "_value"))
w.value = 9
print(w.value)

show("class access raises AttributeError", lambda: Widget.value)
show("class access readonly", lambda: Widget.readonly)
show("class access via classmethod", lambda: Widget.class_level())
show("set readonly", lambda: setattr(w, "readonly", 1))
show("delete readonly", lambda: delattr(w, "readonly"))
print(w.readonly)
print(Widget.__dict__["value"].__doc__)
print(type(Widget.__dict__["value"]).__name__)


# Bare AttributeError() from class access.
try:
    DCA(lambda self: 1).__get__(None, Widget)
except AttributeError as exc:
    print(type(exc).__name__, exc.args, str(exc) == "")

try:
    DCA().__get__(None)
except AttributeError as exc:
    print(exc.args)

# Unreadable / unsettable / undeletable.
empty_dca = DCA()
show("unreadable", lambda: empty_dca.__get__(object(), object))
show("unsettable", lambda: empty_dca.__set__(object(), 1))
show("undeletable", lambda: empty_dca.__delete__(object()))
print(vars(empty_dca))
print(empty_dca.fget, empty_dca.fset, empty_dca.fdel, empty_dca.__doc__)
print(empty_dca.overwrite_doc, empty_dca.__isabstractmethod__)

show("get no args", lambda: empty_dca.__get__())
show("set missing", lambda: empty_dca.__set__(1))
show("delete missing", lambda: empty_dca.__delete__())
show("ctor too many", lambda: DCA(None, None, None, None, None))
show("ctor bad keyword", lambda: DCA(fgot=None))
show("ctor keyword", lambda: DCA(fget=lambda s: 1, doc="given").__doc__)


# Documentation handling.
def documented(self):
    "doc from function"
    return 1


def undocumented(self):
    return 2


d1 = DCA(documented)
print(d1.__doc__, d1.overwrite_doc)
d2 = DCA(documented, doc="explicit")
print(d2.__doc__, d2.overwrite_doc)
d3 = DCA(undocumented)
print(d3.__doc__, d3.overwrite_doc)
d4 = DCA(documented, doc="")
print(repr(d4.__doc__), d4.overwrite_doc)
show("fget without __doc__", lambda: type(DCA(fget=5)).__name__)
d5 = d3.getter(documented)
print(d5.__doc__, d5.overwrite_doc, d5.fget is documented)
d6 = d2.getter(documented)
print(d6.__doc__, d6.overwrite_doc)
d7 = d1.setter(lambda self, v: None)
print(d7.__doc__, d7.overwrite_doc, d7.fget is documented, d7.fset is not None, d7.fdel)
d8 = d1.deleter(lambda self: None)
print(d8.__doc__, d8.overwrite_doc, d8.fget is documented, d8.fset, d8.fdel is not None)
print(d1.getter(undocumented).__doc__)
print(d1 is not d1.getter(documented), d1 is not d1.setter(None), d1 is not d1.deleter(None))
show("getter no args", lambda: d1.getter())
show("setter two args", lambda: d1.setter(1, 2))

# The decorated copies keep the type (subclass-friendly).
class MyDCA(DCA):
    pass


md = MyDCA(documented)
print(type(md.getter(undocumented)).__name__, type(md.setter(None)).__name__,
      type(md.deleter(None)).__name__)

# Abstract-method support.
import abc


class Abstract(abc.ABC):
    @DCA
    @abc.abstractmethod
    def name(self):
        raise NotImplementedError


print(Abstract.__dict__["name"].__isabstractmethod__)
print(Abstract.name is Abstract.__dict__["name"])
print(Abstract.__abstractmethods__ == frozenset({"name"}))
show("abstract instantiate", lambda: Abstract())
print(DCA(lambda self: 1).__isabstractmethod__)
print(DCA(abc.abstractmethod(lambda self: 1)).__isabstractmethod__)


class FakeAbstract:
    __isabstractmethod__ = ["truthy"]


print(DCA(FakeAbstract()).__isabstractmethod__)

# Subclass hooks (what enum.property relies on).
class Recording(DCA):
    def __init__(self, fget=None, fset=None, fdel=None, doc=None):
        super().__init__(fget, fset, fdel, doc)
        self.member = None
        self.calls = []

    def __get__(self, instance, ownerclass=None):
        self.calls.append(("get", instance is None))
        if instance is None:
            if self.member is not None:
                return self.member
            raise AttributeError("class access: " + self.fget.__name__)
        return super().__get__(instance, ownerclass)

    def __set__(self, instance, value):
        self.calls.append(("set", value))
        super().__set__(instance, value)


class UsesRecording:
    def __init__(self):
        self._v = 0

    @Recording
    def v(self):
        return self._v

    @v.setter
    def v(self, new):
        self._v = new


rec = UsesRecording()
rec.v = 4
print(rec.v)
show("recording class access", lambda: UsesRecording.v)
print(UsesRecording.__dict__["v"].calls)
print(type(UsesRecording.__dict__["v"]).__name__, UsesRecording.__dict__["v"].member)

# DynamicClassAttribute used with Enum (enum.property derives from it).
class Color(enum.Enum):
    RED = 1
    GREEN = 2

    @types.DynamicClassAttribute
    def label(self):
        return self.name.lower()


print(Color.RED.name, Color.RED.value, Color.GREEN.label)
show("enum class-level DCA", lambda: Color.label)
print(isinstance(enum.Enum.__dict__["name"], types.DynamicClassAttribute))
print(isinstance(enum.Enum.__dict__["value"], types.DynamicClassAttribute))
print(isinstance(Color.__dict__["label"], types.DynamicClassAttribute))
print(list(Color), Color(2), Color["RED"])
print(enum.property.__mro__[1] is types.DynamicClassAttribute)


class Number(enum.IntEnum):
    ONE = 1
    TWO = 2


print(Number.ONE + 1, Number.TWO.name, Number(1).value)
print(Color.__members__.keys())

# DynamicClassAttribute in a class with __slots__ / property-like use via
# direct descriptor calls.
direct = DCA(lambda self: ("get", self), lambda self, v: ("set", self, v), lambda self: ("del", self))
sentinel = object()
print(direct.__get__(sentinel, type(sentinel))[0], direct.__set__(sentinel, 3), direct.__delete__(sentinel))
print(direct.__get__(sentinel)[1] is sentinel)
getter_raises = DCA(lambda self: 1 / 0)
show("fget error propagates", lambda: getter_raises.__get__(sentinel))
setter_raises = DCA(lambda self: 1, lambda self, v: (_ for _ in ()).throw(KeyError(v)))
show("fset error propagates", lambda: setter_raises.__set__(sentinel, "k"))


# ── coroutine ───────────────────────────────────────────────────────────────
show("coroutine non-callable", lambda: types.coroutine(1))
show("coroutine None", lambda: types.coroutine(None))
show("coroutine no args", lambda: types.coroutine())
show("coroutine two args", lambda: types.coroutine(1, 2))
show("coroutine keyword", lambda: types.coroutine(func=lambda: 0).__name__)
show("coroutine bad keyword", lambda: types.coroutine(fn=lambda: 0))


async def native_coroutine():
    return "native"


print(types.coroutine(native_coroutine) is native_coroutine)
print(native_coroutine.__code__.co_flags & 0x100)


def plain_function():
    return 3


wrapped_plain = types.coroutine(plain_function)
print(wrapped_plain is plain_function, wrapped_plain.__name__, wrapped_plain.__wrapped__ is plain_function)
print(wrapped_plain())
print(wrapped_plain.__qualname__, wrapped_plain.__module__ == plain_function.__module__)


def gen_function(n):
    "gen doc"
    for i in range(n):
        got = yield i
    return n


before = gen_function.__code__.co_flags & 0x20, gen_function.__code__.co_flags & 0x100
print(before)
decorated_gen = types.coroutine(gen_function)
print(decorated_gen is gen_function)
print(gen_function.__code__.co_flags & 0x100)
print(types.coroutine(gen_function) is gen_function)  # idempotent


async def drive_legacy():
    return await gen_function(2)


try:
    c = drive_legacy()
    print(c.send(None))
    print(c.send(None))
except StopIteration as stop:
    print("returned", stop.value)


@types.coroutine
def yields_values():
    sent = yield "first"
    sent2 = yield ("second", sent)
    return ("done", sent2)


async def drive_values():
    return await yields_values()


c = drive_values()
print(c.send(None))
print(c.send("A"))
try:
    c.send("B")
except StopIteration as stop:
    print(stop.value)

# throw / close on an awaited legacy coroutine.
@types.coroutine
def guarded():
    try:
        yield "waiting"
    except ValueError as exc:
        yield ("caught", str(exc))
    finally:
        guarded_events.append("finally")


guarded_events = []


async def drive_guarded():
    return await guarded()


c = drive_guarded()
print(c.send(None))
print(c.throw(ValueError("thrown")))
c.close()
print(guarded_events)

# A non-function callable goes through the wrapper path.
class CallableGen:
    def __call__(self, n):
        for i in range(n):
            yield i * 10
        return "callable-done"


wrapped_callable = types.coroutine(CallableGen())
print(type(wrapped_callable).__name__ != "CallableGen")
gen_obj = wrapped_callable(2)
print(type(gen_obj) is types.GeneratorType)
print(list(gen_obj))


class CallableReturningList:
    def __call__(self):
        return [1, 2, 3]


print(types.coroutine(CallableReturningList())())

# functools.wraps metadata and __wrapped__.
def meta_source(a, b=2, *, c=3):
    "meta doc"
    return a + b + c


meta_source.custom_attr = "custom"
meta_wrapped = types.coroutine(meta_source)
print(meta_wrapped is meta_source)
wrapped_lambda = types.coroutine(lambda: 41)
print(wrapped_lambda.__name__, wrapped_lambda(), wrapped_lambda.__wrapped__() == 41)


class MethodHolder:
    def regular(self, x):
        return ("regular", x)

    def make_gen(self):
        return GeneratorLike()


class GeneratorLike:
    def __init__(self):
        self.step = 0
        self.thrown = []
        self.closed = False
        self.__name__ = "generator_like"
        self.__qualname__ = "GeneratorLike.generator_like"

    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        self.step += 1
        if self.step == 1:
            return "first"
        raise StopIteration(("value", value))

    def throw(self, typ, *rest):
        self.thrown.append((getattr(typ, "__name__", typ), rest))
        return "thrown"

    def close(self):
        self.closed = True
        return "closed"


class Decorated:
    @types.coroutine
    def method(self, x):
        return ("method", self is not None, x)

    @types.coroutine
    def make(self):
        return GeneratorLike()

    @classmethod
    @types.coroutine
    def cm(cls, x):
        return ("classmethod", cls.__name__, x)

    @staticmethod
    @types.coroutine
    def sm(x):
        return ("staticmethod", x)


inst = Decorated()
print(inst.method(1))
print(Decorated.cm(2))
print(Decorated.sm(3))
print(Decorated.method(inst, 4))
wrapper_obj = inst.make()
print(type(wrapper_obj) is types._GeneratorWrapper)

# _GeneratorWrapper.
def gl_factory():
    return GeneratorLike()


wrapped_factory = types.coroutine(gl_factory)
underlying_box = []
wrapper = wrapped_factory()
print(type(wrapper) is types._GeneratorWrapper, type(wrapper).__name__, type(wrapper).__module__)
print(wrapper.__name__, wrapper.__qualname__)
print(sorted(vars(wrapper)))
print(vars(wrapper)["_GeneratorWrapper__isgen"])
print(wrapper.__await__() is wrapper, iter(wrapper) is wrapper)
print(wrapper.send(None))
show("send finishes", lambda: wrapper.send("v"))
wrapper2 = wrapped_factory()
print(next(wrapper2))
show("next finishes", lambda: next(wrapper2))
wrapper3 = wrapped_factory()
print(wrapper3.throw(ValueError, "msg"), wrapper3.close())
print(wrapper3._GeneratorWrapper__wrapped.thrown, wrapper3._GeneratorWrapper__wrapped.closed)
print(wrapper3.throw(KeyError), wrapper3._GeneratorWrapper__wrapped.thrown[-1])
print(wrapper3.throw(tp=OSError))
show("throw no args", lambda: wrapper3.throw())
show("send no args", lambda: wrapper3.send())
show("close extra", lambda: wrapper3.close(1))
show("wrapper no gi_code", lambda: wrapper3.gi_code)
show("wrapper cr_await", lambda: wrapper3.cr_await)
show("wrapper gi_running", lambda: wrapper3.gi_running)
print([name for name in ("send", "throw", "close", "gi_code", "gi_frame", "gi_running",
                         "gi_yieldfrom", "cr_code", "cr_frame", "cr_running", "cr_await",
                         "__next__", "__iter__", "__await__") if name in vars(types._GeneratorWrapper)])
print(isinstance(wrapper, __import__("collections.abc").abc.Generator))
print(isinstance(wrapper, __import__("collections.abc").abc.Coroutine))
print(isinstance(wrapper, __import__("collections.abc").abc.Awaitable))


# _GeneratorWrapper around real generators (the __isgen path).
def plain_generator_factory():
    yield 1
    yield 2


generator_wrapper = types._GeneratorWrapper(plain_generator_factory())
print(vars(generator_wrapper)["_GeneratorWrapper__isgen"])
print(type(iter(generator_wrapper)) is types.GeneratorType, iter(generator_wrapper) is generator_wrapper)
print(type(generator_wrapper.__await__()) is types.GeneratorType)
print(next(generator_wrapper), generator_wrapper.send(None))
print(generator_wrapper.__name__, generator_wrapper.__qualname__.endswith("plain_generator_factory"))
print(type(generator_wrapper.gi_code).__name__, generator_wrapper.gi_running, generator_wrapper.gi_yieldfrom)
print(generator_wrapper.cr_running, generator_wrapper.cr_await is generator_wrapper.gi_yieldfrom)
print(generator_wrapper.close())

no_name = types._GeneratorWrapper(object())
print(no_name.__name__, no_name.__qualname__, vars(no_name)["_GeneratorWrapper__isgen"])
show("wrapper init no args", lambda: types._GeneratorWrapper())
show("wrapper init two", lambda: types._GeneratorWrapper(1, 2))
show("wrapper next on object", lambda: next(no_name))


# Objects already native coroutines / marked generators pass through.
native_made = []


def returns_native():
    coro = native_coroutine()
    native_made.append(coro)
    return coro


rn = types.coroutine(returns_native)()
print(rn is native_made[0], type(rn) is types.CoroutineType)
rn.close()

marked_made = []


def returns_marked():
    gen = decorated_gen(1)
    marked_made.append(gen)
    return gen


rm = types.coroutine(returns_marked)()
print(rm is marked_made[0], type(rm) is types.GeneratorType)
rm.close()


def returns_unmarked_generator():
    return plain_generator_factory()


um = types.coroutine(returns_unmarked_generator)()
print(type(um) is types._GeneratorWrapper)
print(next(um))


class PseudoCoroutine:
    """Implements the Coroutine ABC protocol: passed through unchanged."""

    def __await__(self):
        yield "pseudo"

    def send(self, v):
        return v

    def throw(self, *a):
        raise a[0]

    def close(self):
        pass


pc = PseudoCoroutine()
print(types.coroutine(lambda: pc)() is pc)

for label, product in (
    ("int", 17),
    ("none", None),
    ("list", [1]),
    ("iterator", iter([1, 2])),
    ("tuple", (1, 2)),
):
    print(label, types.coroutine(lambda product=product: product)() is product)


async def agen():
    yield 1


ag = agen()
print(types.coroutine(lambda: ag)() is ag)

# Await protocol through the wrapper.
@types.coroutine
def proto_factory():
    return GeneratorLike()


async def awaits_protocol():
    return await proto_factory()


c = awaits_protocol()
print(c.send(None))
try:
    c.send("sent value")
except StopIteration as stop:
    print(stop.value)

# Exceptions inside the wrapped callable propagate untouched.
def raising_callable():
    raise RuntimeError("inside")


show("wrapped raises", lambda: types.coroutine(raising_callable)())
show("wrapped bad call", lambda: types.coroutine(raising_callable)(1))
show("wrapped kwargs", lambda: types.coroutine(lambda **kw: sorted(kw))(b=1, a=2))
show("wrapped varargs", lambda: types.coroutine(lambda *a: a)(1, 2, 3))

# Decorating a callable class instance keeps the original callable intact.
callable_obj = CallableReturningList()
decorated_callable = types.coroutine(callable_obj)
print(decorated_callable.__wrapped__ is callable_obj)
print(callable_obj() == [1, 2, 3])


# ── SimpleNamespace ─────────────────────────────────────────────────────────
NS = types.SimpleNamespace
print(NS, NS.__module__, NS.__name__, NS.__qualname__)
print(NS.__doc__)
print(NS.__hash__)
print(NS.__eq__ is not object.__eq__)

ns = NS()
print(ns, repr(ns), vars(ns), ns.__dict__)
ns = NS(a=1, b="two", c=[3])
print(ns)
print(ns.a, ns.b, ns.c, vars(ns))
ns.d = 4
ns.a = 10
del ns.b
print(ns)
print(hasattr(ns, "b"), getattr(ns, "b", "gone"))
show("missing attribute", lambda: ns.missing)
show("delete missing", lambda: delattr(ns, "missing"))
print(ns.__dict__ is vars(ns))
ns.__dict__["injected"] = "yes"
print(ns.injected, ns)

# Construction rules.
show("positional arg", lambda: NS(1))
show("positional args", lambda: NS(1, 2))
show("mapping positional", lambda: NS({"a": 1}))
show("keyword splat", lambda: NS(**{"x": 1, "y": 2}))
show("self keyword", lambda: NS(self=1))
show("non-str splat", lambda: NS(**{1: 2}))
print(NS(a=1, **{"b": 2}))
print(NS(**dict(zip("abc", range(3)))))
print(NS(**{"not valid identifier": 1, "ünï": 2}))

reinit = NS(a=1)
reinit.__init__(b=2, a=3)
print(reinit)
show("reinit positional", lambda: reinit.__init__(1))
print(NS.__new__(NS))

# Equality and hashing.
print(NS(a=1) == NS(a=1), NS(a=1) != NS(a=1))
print(NS(a=1) == NS(a=2), NS(a=1) != NS(a=2))
print(NS(a=1, b=2) == NS(b=2, a=1))
print(NS() == NS(), NS() != NS())
print(NS(a=1) == {"a": 1}, NS(a=1) != {"a": 1}, {"a": 1} == NS(a=1))
print(NS(a=1) == 1, NS(a=1) is not None, NS(a=1) == None)
print(NS(a=NS(b=2)) == NS(a=NS(b=2)))
print(NS(a=[1, 2]) == NS(a=[1, 2]), NS(a=[1, 2]) == NS(a=[2, 1]))
print(NS.__eq__(NS(a=1), 5), NS.__ne__(NS(a=1), 5))
print(NS.__eq__(NS(a=1), NS(a=1)), NS.__ne__(NS(a=1), NS(a=1)))
show("hash", lambda: hash(NS()))
show("set member", lambda: {NS()})
show("dict key", lambda: {NS(): 1})
show("less than", lambda: NS(a=1) < NS(a=2))

# Nested and recursive repr.
nested = NS(inner=NS(deep=NS(x=1)), items=[NS(k=1), NS(k=2)])
print(nested)
print(str(nested), format(nested), "%s" % nested, f"{nested!r}")
rec_ns = NS(name="rec")
rec_ns.self = rec_ns
print(rec_ns)
rec_a = NS(label="a")
rec_b = NS(label="b", other=rec_a)
rec_a.other = rec_b
print(rec_a)
print(rec_b)
rec_list = NS(items=[])
rec_list.items.append(rec_list)
print(rec_list)
rec_dict = NS(mapping={})
rec_dict.mapping["me"] = rec_dict
print(rec_dict)
print(NS(x=NS(y=NS(z=NS(w=NS(v=1))))))
print(repr(NS(s="quote'd", t='dq"', u="new\nline", v=b"bytes", w=1.5, x=None, y=True, z=(1,))))
# Attribute order follows insertion.
ordered = NS()
for key in ("z", "a", "m"):
    setattr(ordered, key, key.upper())
print(ordered)
del ordered.a
ordered.a = "again"
print(ordered)

# Keys that are not rendered: empty string.
odd_keys = NS(a=1)
odd_keys.__dict__[""] = "empty"
print(odd_keys, len(vars(odd_keys)))

# Subclassing.
class Point(NS):
    pass


p = Point(x=1, y=2)
print(p, repr(p), type(p).__name__)
print(p == NS(x=1, y=2), NS(x=1, y=2) == p, p != NS(x=1, y=2))
print(Point(x=1) == Point(x=1))
print(isinstance(p, NS), issubclass(Point, NS))
p.z = 3
print(p, vars(p))
recursive_point = Point(label="p")
recursive_point.me = recursive_point
print(recursive_point)


class Custom(NS):
    def __init__(self, base, **kwds):
        super().__init__(**kwds)
        self.base = base

    def __repr__(self):
        return "Custom<%s>" % super().__repr__()


c = Custom(5, extra=1)
print(c, vars(c), [c])


class Slotty(NS):
    __slots__ = ("extra",)


slotty = Slotty(a=1)
slotty.extra = 2
print(slotty, slotty.extra)


class Tracking(NS):
    def __setattr__(self, name, value):
        tracking_log.append(name)
        super().__setattr__(name, value)


tracking_log = []
tracked = Tracking(a=1)
tracked.b = 2
print(tracking_log, tracked)

class NoEq(NS):
    __eq__ = object.__eq__
    __hash__ = object.__hash__


print(NoEq(a=1) == NoEq(a=1), hash(NoEq()) is not None)

# copy / pickle support through __reduce__.
import copy
import pickle

original = NS(a=1, b=[2, 3], c=NS(d=4))
print(original.__reduce__()[0] is NS, original.__reduce__()[1], original.__reduce__()[2] == original.__dict__)
shallow = copy.copy(original)
print(shallow, shallow == original, shallow is original, shallow.b is original.b)
deep = copy.deepcopy(original)
print(deep, deep == original, deep.b is original.b, deep.c is original.c)
round_trip = pickle.loads(pickle.dumps(original))
print(round_trip, round_trip == original, type(round_trip) is NS)
point_trip = pickle.loads(pickle.dumps(Point(x=1)))
print(copy.copy(Point(x=1)), type(copy.copy(Point(x=1))).__name__)


# Error messages that mention the type.
show("add", lambda: NS() + 1)
show("call", lambda: NS()())
show("len", lambda: len(NS()))
show("getitem", lambda: NS()["a"])
show("setitem", lambda: NS().__setitem__)
show("bool", lambda: bool(NS()))
show("slots dir", lambda: sorted(vars(NS(a=1))))

# Attribute names of the class itself.
for name in ("__init__", "__repr__", "__eq__", "__ne__", "__reduce__", "__hash__", "__doc__"):
    print(name, name in vars(NS))

# DynamicClassAttribute and SimpleNamespace error wording.
show("dca attr", lambda: DCA().nope)
show("dca add", lambda: DCA() + 1)


# ── MappingProxyType ────────────────────────────────────────────────────────
backing = {"a": 1, "b": 2}
mp = types.MappingProxyType(backing)
print(mp, repr(mp), len(mp), mp["a"], "a" in mp, "z" in mp, list(mp), list(mp.items()))
backing["c"] = 3
print(mp, len(mp), mp.get("c"), mp.get("zz", "default"), dict(mp))
show("mp setitem", lambda: mp.__setitem__("k", 1))
show("mp delitem", lambda: mp.__delitem__("a"))
show("mp wrong type", lambda: types.MappingProxyType([1]))
show("mp copy", lambda: mp.copy())
print(mp == backing, mp != backing)
print(types.MappingProxyType(mp) is not mp, types.MappingProxyType(mp)["a"])
print(type(mp) is types.MappingProxyType, type(int.__dict__) is types.MappingProxyType)
print(types.MappingProxyType.__name__, types.MappingProxyType.__module__)
print(list(reversed(mp)))
show("mp hash", lambda: hash(mp) is not None)
show("mp keys", lambda: list(mp.keys()))
show("mp values", lambda: list(mp.values()))
show("mp bool", lambda: (bool(mp), bool(types.MappingProxyType({}))))


# ── module-level function metadata ──────────────────────────────────────────
for fn in (types.new_class, types.resolve_bases, types.prepare_class,
           types.get_original_bases, types.coroutine, types._calculate_meta):
    print(fn.__name__, callable(fn))
print([n for n in ("SimpleNamespace", "DynamicClassAttribute", "_GeneratorWrapper") if getattr(types, n).__module__ == "types"])
print(sorted(n for n in dir(types) if n.startswith("_") and not n.startswith("__")))

# Re-importing / reloading types is stable.
import importlib

reloaded = importlib.reload(types)
print(reloaded is types, types.SimpleNamespace(a=1))
print(types.coroutine(lambda: 5)())
print(types.new_class("AfterReload").__name__)
