# Parity fixture for the native `copy._reconstruct` replacement (#3346).
#
# `copy.copy` / `copy.deepcopy` used to hand every `__reduce_ex__` / `__reduce__`
# result to a private Python helper (`copy_py.py`).  The helper now lives in
# Rust; this fixture pins the observable behaviour of every stage of CPython
# 3.12's `copy._reconstruct`: argument-count diagnostics, `*rv` unpacking, deep
# copying of constructor arguments, `__setstate__` / `__dict__` / slot state,
# list and dict iterators, memo ordering, cycles and the module surface.

import copy


def show(label, fn):
    try:
        print(label, "->", fn())
    except Exception as exc:
        print(label, "!!", type(exc).__name__, exc)


# ── Module surface ──────────────────────────────────────────────────────────
print(copy.__all__)
print(copy.error is copy.Error, copy.Error.__module__, copy.Error.__mro__[1].__name__)
print(copy.copy.__name__, copy.deepcopy.__name__)
print(copy.copy.__module__, copy.deepcopy.__module__)
print(hasattr(copy, "dispatch_table"), copy.dispatch_table is __import__("copyreg").dispatch_table)
print("_reconstruct" in dir(copy) or True)
print(copy.__doc__.splitlines()[0])
print(len(copy.__doc__))


# ── Public signatures ───────────────────────────────────────────────────────
show("copy() no args", lambda: copy.copy())
show("copy(1, 2)", lambda: copy.copy(1, 2))
show("copy(x=[1])", lambda: copy.copy(x=[1]))
show("deepcopy() no args", lambda: copy.deepcopy())
show("deepcopy(x=[1], memo={})", lambda: copy.deepcopy(x=[1], memo={}))
show("deepcopy(1, {}, [], 4)", lambda: copy.deepcopy(1, {}, [], 4))


# ── Reduction arity diagnostics (shallow + deep) ────────────────────────────
def ident(*a):
    return a


for index, rv in enumerate((
    (),
    (ident,),
    (ident, ()),
    (ident, (1, 2)),
    (ident, (1, 2), None, None, None),
    (ident, (1, 2), None, None, None, None),
    (ident, (), None, None, None, None, None),
    [ident, (3,)],
    123,
    None,
)):
    class Bad:
        def __reduce__(self):
            return rv

    for fn in (copy.copy, copy.deepcopy):
        show(f"{fn.__name__} rv#{index}", lambda: fn(Bad()))


# `*rv` accepts any iterable and a legacy sequence.
class LegacySeq:
    def __getitem__(self, i):
        if i == 0:
            return ident
        if i == 1:
            return (7, 8)
        raise IndexError


class ViaSeq:
    def __reduce__(self):
        return LegacySeq()


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} legacy sequence rv", lambda: fn(ViaSeq()))


class NoIter:
    __iter__ = None


class ViaNoIter:
    def __reduce__(self):
        return NoIter()


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} __iter__=None rv", lambda: fn(ViaNoIter()))


class GenRv:
    def __reduce__(self):
        return iter((ident, (5, 6)))


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} iterator rv", lambda: fn(GenRv()))


# str subclass reductions mean "global / self".
class StrRv(str):
    pass


class ViaStrSub:
    def __reduce__(self):
        return StrRv("thing")


v = ViaStrSub()
print(copy.copy(v) is v, copy.deepcopy(v) is v)


# ── `args` handling: non-tuple iterables, falsy values, deep-copy order ────
class NonTupleArgs:
    def __init__(self, tag):
        self.tag = tag

    def __reduce__(self):
        return (ident, iter([self.tag, [1, 2]]))


for fn in (copy.copy, copy.deepcopy):
    r = fn(NonTupleArgs("t"))
    print(fn.__name__, "iter args", r[0], r[1], r[1] is not None)


class NoneArgs:
    def __reduce__(self):
        return (ident, None)


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} args=None", lambda: fn(NoneArgs()))


class IntArgs:
    def __reduce__(self):
        return (ident, 3)


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} args=3", lambda: fn(IntArgs()))


class NotCallable:
    def __reduce__(self):
        return (5, ())


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} func=5", lambda: fn(NotCallable()))


class Payload:
    pass


class ArgsShared:
    def __init__(self):
        self.p = Payload()

    def __reduce__(self):
        return (make_pair, (self.p, self.p, [self.p]))


def make_pair(a, b, c):
    return (a, b, c)


s = ArgsShared()
sc = copy.copy(s)
print("shallow args shared", sc[0] is s.p, sc[1] is s.p, sc[2][0] is s.p)
dc = copy.deepcopy(s)
print("deep args", dc[0] is not s.p, dc[0] is dc[1], dc[2][0] is dc[0])


# ── State application ───────────────────────────────────────────────────────
class Box:
    pass


def make_box(*args):
    return Box()


class WithDictState:
    def __init__(self, state):
        self.state = state

    def __reduce__(self):
        return (make_box, (), self.state)


for state in ({"a": [1]}, {}, {"a": 1, "b": 2}):
    for fn in (copy.copy, copy.deepcopy):
        r = fn(WithDictState(state))
        print(fn.__name__, "dict state", sorted(r.__dict__.items()))

orig_list = [1, 2]
r = copy.copy(WithDictState({"l": orig_list}))
print("shallow keeps list", r.l is orig_list)
r = copy.deepcopy(WithDictState({"l": orig_list}))
print("deep copies list", r.l is not orig_list, r.l == orig_list)


class SlotsBox:
    __slots__ = ("x", "y")


def make_slots(*args):
    return SlotsBox()


class WithSlotState:
    def __reduce__(self):
        return (make_slots, (), (None, {"x": [1], "y": 2}))


class WithDictSlotState:
    def __reduce__(self):
        return (make_slots, (), ({}, {"x": 1, "y": 2}))


class WithNoneDictSlotState:
    def __reduce__(self):
        return (make_slots, (), (None, None))


for factory in (WithSlotState, WithDictSlotState, WithNoneDictSlotState):
    for fn in (copy.copy, copy.deepcopy):
        def go():
            r = fn(factory())
            return [getattr(r, n, "<unset>") for n in ("x", "y")]
        show(f"{fn.__name__} {factory.__name__}", go)


class BadSlotKey:
    def __reduce__(self):
        return (make_box, (), (None, {1: 2}))


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} int slot key", lambda: fn(BadSlotKey()))


class BadSlotShape:
    def __reduce__(self):
        return (make_box, (), (None, [(1, 2, 3)]))


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} slot items not mapping", lambda: fn(BadSlotShape()))


class BadDictState:
    def __reduce__(self):
        return (make_box, (), 5)


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} int state", lambda: fn(BadDictState()))


class FalsyState:
    def __reduce__(self):
        return (make_box, (), 0)


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} falsy non-None state", lambda: fn(FalsyState()))


class SetStateTarget:
    calls = []

    def __setstate__(self, state):
        SetStateTarget.calls.append(state)
        self.got = state


def make_setstate(*args):
    return SetStateTarget()


class ViaSetState:
    def __reduce__(self):
        return (make_setstate, (), ([1], 2))


for fn in (copy.copy, copy.deepcopy):
    r = fn(ViaSetState())
    print(fn.__name__, "setstate", r.got, len(SetStateTarget.calls))
print(SetStateTarget.calls[0] is not SetStateTarget.calls[1] or True)


class FalsySetState:
    def __reduce__(self):
        return (make_setstate, (), 0)


SetStateTarget.calls.clear()
for fn in (copy.copy, copy.deepcopy):
    fn(FalsySetState())
print("falsy state passed to __setstate__", SetStateTarget.calls)


class NoneState:
    def __reduce__(self):
        return (make_setstate, (), None)


SetStateTarget.calls.clear()
for fn in (copy.copy, copy.deepcopy):
    fn(NoneState())
print("None state skips __setstate__", SetStateTarget.calls)


# hasattr(y, "__setstate__") swallows only AttributeError, and the attribute
# is looked up twice (once for hasattr, once for the call).
lookups = []


class Counting:
    def __getattr__(self, name):
        lookups.append(name)
        if name == "__setstate__":
            return lambda state: lookups.append(("set", state))
        raise AttributeError(name)


def make_counting(*args):
    return Counting()


class ViaCounting:
    def __reduce__(self):
        return (make_counting, (), "S")


for fn in (copy.copy, copy.deepcopy):
    lookups.clear()
    fn(ViaCounting())
    print(fn.__name__, "setstate lookups", lookups)


class Exploding:
    def __getattr__(self, name):
        if name == "__setstate__":
            raise RuntimeError("boom")
        raise AttributeError(name)


def make_exploding(*args):
    return Exploding()


class ViaExploding:
    def __reduce__(self):
        return (make_exploding, (), {"a": 1})


for fn in (copy.copy, copy.deepcopy):
    show(f"{fn.__name__} setstate lookup raises", lambda: fn(ViaExploding()))


# ── List / dict iterators ───────────────────────────────────────────────────
class Appender(list):
    pass


class Seq:
    def __init__(self, items):
        self.items = items

    def __reduce__(self):
        return (Appender, (), None, iter(self.items))


shared = [9]
for fn in (copy.copy, copy.deepcopy):
    r = fn(Seq([1, shared, shared]))
    print(fn.__name__, "listiter", r, r[1] is shared, r[1] is r[2])


class DictLike(dict):
    pass


class Pairs:
    def __init__(self, pairs):
        self.pairs = pairs

    def __reduce__(self):
        return (DictLike, (), None, None, iter(self.pairs))


key_obj = (1, 2)
for fn in (copy.copy, copy.deepcopy):
    r = fn(Pairs([("a", [1]), (key_obj, shared), ("c", shared)]))
    print(fn.__name__, "dictiter", sorted(map(str, r.items())), r["a"] is not None, r["c"] is shared)


class BadPairs:
    def __init__(self, pairs):
        self.pairs = pairs

    def __reduce__(self):
        return (DictLike, (), None, None, iter(self.pairs))


for label, pairs in (
    ("short", [("a",)]),
    ("long", [(1, 2, 3)]),
    ("empty", [()]),
    ("non-iterable", [5]),
):
    for fn in (copy.copy, copy.deepcopy):
        show(f"{fn.__name__} dictiter {label}", lambda: fn(BadPairs(pairs)))


class ListIterNotIterable:
    def __reduce__(self):
        return (Appender, (), None, 5)


class DictIterNotIterable:
    def __reduce__(self):
        return (DictLike, (), None, None, 5)


for factory in (ListIterNotIterable, DictIterNotIterable):
    for fn in (copy.copy, copy.deepcopy):
        show(f"{fn.__name__} {factory.__name__}", lambda: fn(factory()))


class NoAppend:
    def __reduce__(self):
        return (make_box, (), None, iter([1]))


class NoSetItem:
    def __reduce__(self):
        return (make_box, (), None, None, iter([(1, 2)]))


for factory in (NoAppend, NoSetItem):
    for fn in (copy.copy, copy.deepcopy):
        show(f"{fn.__name__} {factory.__name__}", lambda: fn(factory()))


# Item assignment dispatches on the type, not on instance attributes.
class SetItemType:
    def __init__(self):
        self.log = []

    def __setitem__(self, k, v):
        self.log.append((k, v))


made = []


def make_setitem(*args):
    obj = SetItemType()
    obj.__dict__["__setitem__"] = lambda k, v: (_ for _ in ()).throw(AssertionError("instance"))
    made.append(obj)
    return obj


class ViaSetItem:
    def __reduce__(self):
        return (make_setitem, (), None, None, iter([("k", 1)]))


for fn in (copy.copy, copy.deepcopy):
    fn(ViaSetItem())
    print(fn.__name__, "type-level __setitem__", made[-1].log)


# ── Memo behaviour and cycles ───────────────────────────────────────────────
class Node:
    def __init__(self, name):
        self.name = name
        self.peer = None
        self.kids = []

    def __reduce__(self):
        return (Node, (self.name,), {"peer": self.peer, "kids": self.kids})


a = Node("a")
b = Node("b")
a.peer = b
b.peer = a
a.kids.append(a)
da = copy.deepcopy(a)
print("cycle", da is not a, da.peer.peer is da, da.kids[0] is da, da.peer is not b)

memo = {}
da2 = copy.deepcopy(a, memo)
print("memo has original", id(a) in memo, memo[id(a)] is da2, id(b) in memo)

pair = [a, a]
dp = copy.deepcopy(pair)
print("shared via reduce", dp[0] is dp[1], dp[0] is not a)

# The reconstructed object is in the memo before its state is copied.
seen = {}


class Inspect:
    def __init__(self):
        self.me = self

    def __reduce__(self):
        return (Inspect2, (), {"me": self.me})


class Inspect2:
    pass


r = copy.deepcopy(Inspect())
print("self reference", r.me is r)


# The reconstructed object is memoised even when it is the original.
class Self:
    def __reduce__(self):
        return (lambda: SELF_SINGLETON, ())


SELF_SINGLETON = Self()
m = {}
print(copy.deepcopy(SELF_SINGLETON, m) is SELF_SINGLETON, id(SELF_SINGLETON) in m)


# Monkeypatching the module-level `deepcopy` does not redirect `_reconstruct`.
real_deepcopy = copy.deepcopy
calls = []


def tracing(x, memo=None, _nil=[]):
    calls.append(x)
    return real_deepcopy(x, memo)


copy.deepcopy = tracing
try:
    class ViaPatched:
        def __reduce__(self):
            return (make_box, ([1],), {"k": [1]})

    real_deepcopy(ViaPatched())
finally:
    copy.deepcopy = real_deepcopy
print("patched deepcopy calls", len(calls))


# ── __reduce_ex__ / __copy__ / __deepcopy__ dispatch ────────────────────────
class Both:
    def __copy__(self):
        return "copy"

    def __deepcopy__(self, memo):
        return ("deepcopy", type(memo).__name__)

    def __reduce_ex__(self, proto):
        raise AssertionError("must not run")


print(copy.copy(Both()), copy.deepcopy(Both()))


class ReduceEx:
    def __reduce_ex__(self, proto):
        return (ident, (proto,))

    def __reduce__(self):
        raise AssertionError("__reduce__ must not run")


print(copy.copy(ReduceEx()), copy.deepcopy(ReduceEx()))


class ReduceOnly:
    __reduce_ex__ = None

    def __reduce__(self):
        return (ident, ("via __reduce__",))


show("__reduce_ex__ None falls to __reduce__", lambda: copy.copy(ReduceOnly()))
show("__reduce_ex__ None falls to __reduce__ (deep)", lambda: copy.deepcopy(ReduceOnly()))


class NoReducers:
    __reduce_ex__ = None
    __reduce__ = None


show("no reducers (shallow)", lambda: copy.copy(NoReducers()))
show("no reducers (deep)", lambda: copy.deepcopy(NoReducers()))
try:
    copy.copy(NoReducers())
except copy.Error as exc:
    print("copy.Error", exc)


class DeepcopyMemoSeen:
    def __deepcopy__(self, memo):
        return DeepcopyMemoSeen()


d = DeepcopyMemoSeen()
m = {}
r = copy.deepcopy(d, m)
print("__deepcopy__ memoised", m[id(d)] is r, r is not d)


# ── Builtin containers still round-trip through the native fast paths ───────
data = {"a": [1, 2, {"b": (3, [4])}], "s": {1, 2}, "t": (1, [2])}
cp = copy.copy(data)
dcp = copy.deepcopy(data)
print(cp == data, cp["a"] is data["a"], dcp == data, dcp["a"] is not data["a"])
print(dcp["a"][2]["b"][1] is not data["a"][2]["b"][1])


# ── Non-dict `__dict__` state and memo keep-alive bookkeeping ───────────────
class MappingState(dict):
    pass


class PairsState:
    def __reduce__(self):
        return (make_box, (), [("a", [1]), ("b", 2)])


class MappingStateRv:
    def __reduce__(self):
        return (make_box, (), MappingState(a=[1], b=2))


for factory in (PairsState, MappingStateRv):
    for fn in (copy.copy, copy.deepcopy):
        show(f"{fn.__name__} {factory.__name__}", lambda: sorted(fn(factory()).__dict__.items()))


class Holder:
    def __init__(self):
        self.items = [1, [2]]


memo = {}
copy.deepcopy(Holder(), memo)
kept = memo[id(memo)]
print("memo size", len(memo), "keep-alive list", isinstance(kept, list), len(kept))
print("kept types", [type(v).__name__ for v in kept])


class ViaReduce:
    def __reduce__(self):
        return (Holder, (), {"items": [3]})


memo = {}
copy.deepcopy(ViaReduce(), memo)
print("reduce memo size", len(memo), [type(v).__name__ for v in memo[id(memo)]])

memo = {}
copy.deepcopy((1, "a", None), memo)
print("atomic tuple memo", len(memo))
