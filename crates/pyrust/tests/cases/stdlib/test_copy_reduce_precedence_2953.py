# copy module — user-defined __reduce__ / __reduce_ex__ support and precedence
# in copy.copy and copy.deepcopy (issue #2953).

import copy
from collections import Counter, OrderedDict, defaultdict, deque


# ── 1. Precedence: __copy__ / __deepcopy__ > __reduce_ex__ > __reduce__ > default ─


class AllHooks:
    def __init__(self, tag):
        self.tag = tag

    def __copy__(self):
        return AllHooks("copy")

    def __deepcopy__(self, memo):
        return AllHooks("deepcopy")

    def __reduce_ex__(self, proto):
        return (AllHooks, (f"reduce_ex:{proto}",))

    def __reduce__(self):
        return (AllHooks, ("reduce",))


assert copy.copy(AllHooks("orig")).tag == "copy"
assert copy.deepcopy(AllHooks("orig")).tag == "deepcopy"


class ReduceExAndReduce:
    def __init__(self, tag):
        self.tag = tag

    def __reduce_ex__(self, proto):
        return (ReduceExAndReduce, (f"reduce_ex:{proto}",))

    def __reduce__(self):
        return (ReduceExAndReduce, ("reduce",))


assert copy.copy(ReduceExAndReduce("orig")).tag == "reduce_ex:4"
assert copy.deepcopy(ReduceExAndReduce("orig")).tag == "reduce_ex:4"


class OnlyReduce:
    def __init__(self, v):
        self.v = v

    def __reduce__(self):
        return (OnlyReduce, (self.v + 1,))


assert copy.copy(OnlyReduce(10)).v == 11
assert copy.deepcopy(OnlyReduce(10)).v == 11


class ParentReduceEx:
    def __init__(self, v):
        self.v = v

    def __reduce_ex__(self, proto):
        return (type(self), (self.v + proto * 10,))


class ChildReduce(ParentReduceEx):
    def __reduce__(self):
        return (type(self), (self.v + 1,))


# Parent's custom __reduce_ex__ still wins over child's __reduce__ in CPython.
assert copy.copy(ChildReduce(5)).v == 45
assert copy.deepcopy(ChildReduce(5)).v == 45


# __copy__ = None / __deepcopy__ = None fall through to __reduce_ex__ / __reduce__.
class NoneCopyHooks:
    __copy__ = None
    __deepcopy__ = None

    def __init__(self, v):
        self.v = v

    def __reduce__(self):
        return (NoneCopyHooks, (self.v + 7,))


assert copy.copy(NoneCopyHooks(3)).v == 10
assert copy.deepcopy(NoneCopyHooks(3)).v == 10


# __reduce_ex__ = None falls through to __reduce__.
class DisableReduceEx:
    __reduce_ex__ = None

    def __init__(self, v):
        self.v = v

    def __reduce__(self):
        return (DisableReduceEx, (self.v + 9,))


assert copy.copy(DisableReduceEx(1)).v == 10
assert copy.deepcopy(DisableReduceEx(1)).v == 10


# __reduce_ex__ = None without overriding __reduce__ uses default copy protocol.
class ReduceExNoneDefault:
    __reduce_ex__ = None

    def __init__(self, v):
        self.v = v


assert copy.copy(ReduceExNoneDefault(42)).v == 42
assert copy.deepcopy(ReduceExNoneDefault(42)).v == 42


# Disabling both __reduce_ex__ and __reduce__ raises copy.Error.
class Uncopyable:
    __reduce_ex__ = None
    __reduce__ = None


for fn, expected_kind in ((copy.copy, "shallow"), (copy.deepcopy, "deep")):
    try:
        fn(Uncopyable())
        assert False, f"{fn.__name__} should have raised copy.Error"
    except copy.Error as exc:
        assert f"un({expected_kind})copyable object of type" in str(exc), str(exc)


# __reduce__ = None (without disabling __reduce_ex__) fails when object.__reduce_ex__
# delegates to self.__reduce__().
class ReduceNoneOnly:
    __reduce__ = None


for fn in (copy.copy, copy.deepcopy):
    try:
        fn(ReduceNoneOnly())
        assert False, f"{fn.__name__} should have raised TypeError"
    except TypeError as exc:
        assert str(exc) == "'NoneType' object is not callable", str(exc)


# ── 2. Instance-level vs class-level hooks & descriptor kinds ─────────────────


class InstHooks:
    def __init__(self, v):
        self.v = v


# copy.copy looks up __copy__ on the CLASS; copy.deepcopy looks up __deepcopy__
# on the INSTANCE.
ih = InstHooks(10)
ih.__copy__ = lambda: InstHooks(99)
ih.__deepcopy__ = lambda memo: InstHooks(88)
assert copy.copy(ih).v == 10
assert copy.deepcopy(ih).v == 88

# Instance-level __reduce_ex__ is honoured by both copy and deepcopy.
ih2 = InstHooks(10)
ih2.__reduce_ex__ = lambda proto: (InstHooks, (70 + proto,))
assert copy.copy(ih2).v == 74
assert copy.deepcopy(ih2).v == 74

# Instance-level __reduce__ on a class that does NOT override __reduce__ is
# ignored by object.__reduce_ex__; when the class DOES override __reduce__,
# object.__reduce_ex__ dispatches getattr(self, "__reduce__")().
plain_inst = InstHooks(5)
plain_inst.__reduce__ = lambda: (InstHooks, (500,))
assert copy.copy(plain_inst).v == 5
assert copy.deepcopy(plain_inst).v == 5

inst_override = OnlyReduce(5)
inst_override.__reduce__ = lambda: (OnlyReduce, (500,))
assert copy.copy(inst_override).v == 500
assert copy.deepcopy(inst_override).v == 500

# Instance-level __reduce_ex__ = None + instance __reduce__ falls through.
inst_disable = InstHooks(5)
inst_disable.__reduce_ex__ = None
inst_disable.__reduce__ = lambda: (InstHooks, (600,))
assert copy.copy(inst_disable).v == 600
assert copy.deepcopy(inst_disable).v == 600


# Dynamic __getattribute__ supplying __reduce_ex__.
class DynamicReduceEx:
    def __init__(self, v):
        self.v = v

    def __getattribute__(self, name):
        if name == "__reduce_ex__":
            return lambda proto: (DynamicReduceEx, (proto * 100,))
        return object.__getattribute__(self, name)


assert copy.copy(DynamicReduceEx(1)).v == 400
assert copy.deepcopy(DynamicReduceEx(1)).v == 400


# staticmethod / classmethod reducers and __deepcopy__ hooks.
class StaticAndClassReducers:
    def __init__(self, v):
        self.v = v

    @staticmethod
    def __reduce__():
        return (StaticAndClassReducers, (123,))


assert copy.copy(StaticAndClassReducers(0)).v == 123
assert copy.deepcopy(StaticAndClassReducers(0)).v == 123


class ClassMethodReduceEx:
    def __init__(self, v):
        self.v = v

    @classmethod
    def __reduce_ex__(cls, proto):
        return (cls, (proto + 100,))


assert copy.copy(ClassMethodReduceEx(0)).v == 104
assert copy.deepcopy(ClassMethodReduceEx(0)).v == 104


class StaticDeepHook:
    @staticmethod
    def __deepcopy__(memo):
        return ("static_deep", isinstance(memo, dict))


class ClassDeepHook:
    @classmethod
    def __deepcopy__(cls, memo):
        return ("class_deep", cls.__name__, isinstance(memo, dict))


assert copy.deepcopy(StaticDeepHook()) == ("static_deep", True)
assert copy.deepcopy(ClassDeepHook()) == ("class_deep", "ClassDeepHook", True)


# ── 3. Full _reconstruct semantics (str, 2..5-tuples, state, slots, iters) ────


# String return -> original object returned unchanged, NOT memoized by deepcopy.
class Singleton:
    def __reduce__(self):
        return "SINGLETON_GLOBAL"


s = Singleton()
m = {}
assert copy.copy(s) is s
assert copy.deepcopy(s, m) is s
assert len(m) == 0


# __deepcopy__ returning self is also not memoized.
class ReturnSelfDeep:
    def __deepcopy__(self, memo):
        return self


rs = ReturnSelfDeep()
m2 = {}
assert copy.deepcopy(rs, m2) is rs
assert len(m2) == 0


# Full 5-tuple reconstruction: (func, args, state, listiter, dictiter)
class Container:
    def __init__(self, head):
        self.head = head
        self.seq = []
        self.kv = {}

    def append(self, item):
        self.seq.append(item)

    def __setitem__(self, k, v):
        self.kv[k] = v


class Full5:
    def __init__(self):
        self.a = [1]
        self.b = [2]
        self.c = [3]

    def __reduce__(self):
        return (
            Container,
            (self.a,),
            {"st": self.b},
            iter([self.c]),
            iter([("k", self.c)]),
        )


f5 = Full5()
sc5 = copy.copy(f5)
assert sc5.head is f5.a
assert sc5.st is f5.b
assert sc5.seq == [[3]] and sc5.seq[0] is f5.c
assert sc5.kv["k"] is f5.c

dc5 = copy.deepcopy(f5)
assert dc5.head == [1] and dc5.head is not f5.a
assert dc5.st == [2] and dc5.st is not f5.b
assert dc5.seq == [[3]] and dc5.seq[0] is not f5.c
assert dc5.kv["k"] is dc5.seq[0]


# 3-tuple with (dict_state, slot_state) 2-tuple state and None dict_state.
class SlotTarget:
    pass


class TupleStateReducer:
    def __init__(self, payload):
        self.payload = payload

    def __reduce__(self):
        return (SlotTarget, (), ({"d": self.payload}, {"slot_x": self.payload}))


ts = TupleStateReducer([10])
sc_ts = copy.copy(ts)
assert sc_ts.d is ts.payload and sc_ts.slot_x is ts.payload
dc_ts = copy.deepcopy(ts)
assert dc_ts.d == [10] and dc_ts.d is not ts.payload and dc_ts.slot_x is dc_ts.d


class SlotOnlyStateReducer:
    def __reduce__(self):
        return (SlotTarget, (), (None, {"only_slot": 42}))


assert copy.copy(SlotOnlyStateReducer()).only_slot == 42
assert copy.deepcopy(SlotOnlyStateReducer()).only_slot == 42


# 3-tuple with custom __setstate__ (even when state is falsy but not None).
class SetStateTarget:
    calls = []

    def __setstate__(self, state):
        SetStateTarget.calls.append(state)
        self.got = state


class EmptyDictStateReducer:
    def __reduce__(self):
        return (SetStateTarget, (), {})


SetStateTarget.calls.clear()
assert copy.copy(EmptyDictStateReducer()).got == {}
assert copy.deepcopy(EmptyDictStateReducer()).got == {}
assert SetStateTarget.calls == [{}, {}]


# Non-tuple iterable return (list / generator) from __reduce__.
class ListReturnReduce:
    def __reduce__(self):
        return [SlotTarget, (), {"from_list": 7}]


assert copy.copy(ListReturnReduce()).from_list == 7
assert copy.deepcopy(ListReturnReduce()).from_list == 7


# Cycle safety in deepcopy: memo[id(x)] = y BEFORE applying state/listiter/dictiter.
class Cyclic:
    def __init__(self):
        self.ref = None
        self.items = []
        self.mapping = {}

    def append(self, v):
        self.items.append(v)

    def __setitem__(self, k, v):
        self.mapping[k] = v

    def __reduce__(self):
        return (Cyclic, (), {"ref": self}, iter([self]), iter([("self", self)]))


cyc = Cyclic()
cyc.ref = cyc
dcyc = copy.deepcopy(cyc)
assert dcyc is not cyc
assert dcyc.ref is dcyc
assert dcyc.items[0] is dcyc
assert dcyc.mapping["self"] is dcyc


# Invalid reduction returns match CPython's TypeError messages.
for rv, expected_msg in (
    ((), "_reconstruct() missing 2 required positional arguments: 'func' and 'args'"),
    ((SlotTarget,), "_reconstruct() missing 1 required positional argument: 'args'"),
    (
        (SlotTarget, (), None, None, None, None),
        "_reconstruct() takes from 4 to 7 positional arguments but 8 were given",
    ),
    (123, "Value after * must be an iterable, not int"),
):
    class BadReduce:
        def __reduce__(self):
            return rv

    for fn in (copy.copy, copy.deepcopy):
        try:
            fn(BadReduce())
            assert False, f"{fn.__name__}({rv!r}) should have raised TypeError"
        except TypeError as exc:
            assert str(exc) == expected_msg, (fn.__name__, rv, str(exc), expected_msg)


# ── 4. Builtin and Exception subclasses with custom __reduce__ / __reduce_ex__ ─


class CustomDict(dict):
    def __reduce__(self):
        return (CustomDict, ({"reduced": len(self)},))


assert copy.copy(CustomDict(a=1, b=2)) == {"reduced": 2}
assert copy.deepcopy(CustomDict(a=1, b=2)) == {"reduced": 2}


class CustomList(list):
    def __reduce_ex__(self, proto):
        return (CustomList, ([proto, len(self)],))


assert copy.copy(CustomList([1, 2, 3])) == [4, 3]
assert copy.deepcopy(CustomList([1, 2, 3])) == [4, 3]


class CustomSet(set):
    def __reduce__(self):
        return (CustomSet, ([99],))


assert copy.copy(CustomSet([1, 2])) == {99}
assert copy.deepcopy(CustomSet([1, 2])) == {99}


class CustomOD(OrderedDict):
    def __reduce__(self):
        return (CustomOD, ([("k", 1)],))


assert list(copy.copy(CustomOD(a=1)).items()) == [("k", 1)]
assert list(copy.deepcopy(CustomOD(a=1)).items()) == [("k", 1)]


class CustomCounter(Counter):
    def __reduce__(self):
        return (CustomCounter, ({"z": 3},))


assert dict(copy.copy(CustomCounter(a=1))) == {"z": 3}
assert dict(copy.deepcopy(CustomCounter(a=1))) == {"z": 3}


class CustomBytearray(bytearray):
    def __reduce_ex__(self, proto):
        return (CustomBytearray, (b"rx",))


assert bytes(copy.copy(CustomBytearray(b"abc"))) == b"rx"
assert bytes(copy.deepcopy(CustomBytearray(b"abc"))) == b"rx"


class CustomDeque(deque):
    __copy__ = None

    def __reduce__(self):
        return (CustomDeque, ([7, 8],))


assert list(copy.copy(CustomDeque([1, 2]))) == [7, 8]
assert list(copy.deepcopy(CustomDeque([1, 2]))) == [7, 8]


class CustomDefaultDict(defaultdict):
    __copy__ = None

    def __reduce_ex__(self, proto):
        return (CustomDefaultDict, (int, {"p": proto}))


assert dict(copy.copy(CustomDefaultDict(list))) == {"p": 4}
assert dict(copy.deepcopy(CustomDefaultDict(list))) == {"p": 4}


class CustomExc(ValueError):
    def __reduce__(self):
        return (CustomExc, ("from_reduce",))


assert copy.copy(CustomExc("orig")).args == ("from_reduce",)
assert copy.deepcopy(CustomExc("orig")).args == ("from_reduce",)


# Explicit memo=None in copy.deepcopy works like omitting memo.
assert copy.deepcopy([1, [2]], None) == [1, [2]]

# object.__reduce_ex__ / BaseException.__reduce_ex__ delegate to user __reduce__.
assert object.__reduce_ex__(OnlyReduce(10), 4) == (OnlyReduce, (11,))
assert BaseException.__reduce_ex__(CustomExc("orig"), 4) == (CustomExc, ("from_reduce",))

print("copy reduce precedence 2953 ok")
