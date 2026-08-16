import sys
import typing
from collections import ChainMap, UserDict, defaultdict, deque
from types import MappingProxyType


print("--- defaultdict protocols ---")
defaulted = defaultdict(int, {"a": 1})
defaulted_proxy = MappingProxyType(defaulted)
print(defaulted_proxy["absent"], dict(defaulted))
print(defaulted_proxy.get("get-missing"), "get-missing" in defaulted)
print("contains-missing" in defaulted_proxy, "contains-missing" in defaulted)


print("--- dict subclass read routing ---")
calls = []


class RecordingDict(dict):
    def __getitem__(self, key):
        calls.append("__getitem__")
        return "item:" + key

    def __len__(self):
        calls.append("__len__")
        return 99

    def __contains__(self, key):
        calls.append("__contains__")
        return True

    def __iter__(self):
        calls.append("__iter__")
        return iter(["ITER"])

    def get(self, key, default=None):
        calls.append("get")
        return "GET"

    def keys(self):
        calls.append("keys")
        return ["K1", "K2"]

    def values(self):
        calls.append("values")
        return ["V1", "V2"]

    def items(self):
        calls.append("items")
        return [("I1", 1), ("I2", 2)]

    def __reversed__(self):
        calls.append("__reversed__")
        return iter(["REV"])


recording = MappingProxyType(RecordingDict(a=1, b=2))


def record(label, operation):
    calls.clear()
    result = operation()
    seen = list(calls)
    print(label, result, seen)


record("getitem", lambda: recording["a"])
record("get", lambda: recording.get("a"))
record("contains", lambda: "missing" in recording)
record("len", lambda: len(recording))
record("iter", lambda: list(recording))
record("keys", lambda: recording.keys())
record("values", lambda: recording.values())
record("items", lambda: recording.items())
record("reversed", lambda: list(reversed(recording)))
record("dict", lambda: dict(recording))
record("unpack", lambda: {**recording})


def extend_recording():
    result = []
    result.extend(recording)
    return result


record("star-list", lambda: [*recording])
record("star-tuple", lambda: (*recording,))
record("list-extend", extend_recording)
record("sorted", lambda: sorted(recording))


print("--- owner methods use ordinary attribute lookup ---")


class InstanceReads(dict):
    def __getattribute__(self, name):
        if name in ("keys", "values", "items", "get", "copy", "__reversed__"):
            calls.append("getattr:" + name)
        return super().__getattribute__(name)


instance_reads = InstanceReads(a=1)
instance_reads.keys = lambda: ["INSTANCE_KEY"]
instance_reads.values = lambda: ["INSTANCE_VALUE"]
instance_reads.items = lambda: [("INSTANCE_ITEM", 1)]
instance_reads.get = lambda key, default=None: "INSTANCE_GET"
instance_reads.copy = lambda: "INSTANCE_COPY"
instance_reads.__reversed__ = lambda: iter(["INSTANCE_REVERSED"])
instance_proxy = MappingProxyType(instance_reads)
record("instance-keys", lambda: instance_proxy.keys())
record("instance-values", lambda: instance_proxy.values())
record("instance-items", lambda: instance_proxy.items())
record("instance-get", lambda: instance_proxy.get("a"))
record("instance-copy", lambda: instance_proxy.copy())
record("instance-reversed", lambda: list(instance_proxy.__reversed__()))
record("instance-reversed-builtin", lambda: list(reversed(instance_proxy)))


def report_wrapper_error(label, operation):
    calls.clear()
    try:
        operation()
    except TypeError as error:
        print(label, str(error), list(calls))


report_wrapper_error("keys-pos", lambda: instance_proxy.keys(1))
report_wrapper_error("keys-kw", lambda: instance_proxy.keys(z=1))
report_wrapper_error("keys-both", lambda: instance_proxy.keys(1, z=2))
report_wrapper_error("get-empty", lambda: instance_proxy.get())
report_wrapper_error("get-many", lambda: instance_proxy.get(1, 2, 3))
report_wrapper_error("get-kw", lambda: instance_proxy.get(1, z=2))


print("--- sized consumers probe length before consuming ---")
length_events = []


class LengthFailure:
    def __getitem__(self, key):
        raise KeyError(key)

    def __iter__(self):
        length_events.append("__iter__")
        return self

    def __next__(self):
        length_events.append("__next__")
        raise RuntimeError("consumed before length")

    def __len__(self):
        length_events.append("__len__")
        raise RuntimeError("length boom")


length_failure_proxy = MappingProxyType(LengthFailure())


def report_length_failure(label, operation):
    length_events.clear()
    try:
        operation()
    except RuntimeError as error:
        print(label, str(error), list(length_events))


length_extend_target = [9]


def extend_length_failure():
    length_extend_target.extend(length_failure_proxy)


report_length_failure("list", lambda: list(length_failure_proxy))
report_length_failure("star-list", lambda: [*length_failure_proxy])
report_length_failure("star-tuple", lambda: (*length_failure_proxy,))
report_length_failure("list-extend", extend_length_failure)
print("list-extend target", length_extend_target)
report_length_failure("sorted", lambda: sorted(length_failure_proxy))


print("--- bytearray consumers use delegated iteration ---")
byte_events = []


class ByteValues:
    def __getitem__(self, key):
        return key

    def __iter__(self):
        byte_events.append("__iter__")
        return iter([65, 66])

    def __len__(self):
        byte_events.append("__len__")
        return 2


byte_proxy = MappingProxyType(ByteValues())


def report_byte_consumer(label, operation):
    byte_events.clear()
    print(label, operation(), list(byte_events))


def extend_bytearray():
    result = bytearray(b"x")
    result.extend(byte_proxy)
    return result


def assign_bytearray_slice():
    result = bytearray(b"xxxx")
    result[1:3] = byte_proxy
    return result


report_byte_consumer("bytearray-constructor", lambda: bytearray(byte_proxy))
report_byte_consumer("bytearray-extend", extend_bytearray)
report_byte_consumer("bytearray-slice", assign_bytearray_slice)


print("--- proxy truth uses owner length, not owner bool ---")


class LengthAndBool:
    def __getitem__(self, key):
        raise KeyError(key)

    def __len__(self):
        print("owner __len__")
        return 1

    def __bool__(self):
        print("owner __bool__")
        return False


print(bool(MappingProxyType(LengthAndBool())))


print("--- accepted subscript owners ---")


class OnlyGetitem:
    def __getitem__(self, key):
        return "only:" + key


class DisabledGetitem:
    __getitem__ = None


class DequeSubclass(deque):
    pass


class DequeOverride(deque):
    def __getitem__(self, key):
        return "deque-override:" + key


class MappingMixin:
    def __getitem__(self, key):
        return "mixin:" + key


class DequeWithMappingBase(deque, MappingMixin):
    pass


accepted = [
    ("UserDict", UserDict(a=1), lambda proxy: (repr(proxy), len(proxy), proxy["a"])),
    (
        "ChainMap",
        ChainMap({"a": 1}),
        lambda proxy: (repr(proxy), len(proxy), proxy["a"]),
    ),
    ("OnlyGetitem", OnlyGetitem(), lambda proxy: proxy["x"]),
    ("DisabledGetitem", DisabledGetitem(), lambda proxy: type(proxy).__name__),
    ("DequeOverride", DequeOverride(), lambda proxy: proxy["x"]),
    ("DequeWithMappingBase", DequeWithMappingBase([7]), lambda proxy: proxy[0]),
    ("GenericAlias", list[int], lambda proxy: repr(proxy)),
    ("UnionType", int | str, lambda proxy: repr(proxy)),
    ("typing.List", typing.List[int], lambda proxy: repr(proxy)),
    ("typing.Union", typing.Union[int, str], lambda proxy: repr(proxy)),
    ("str", "abc", lambda proxy: (repr(proxy), len(proxy), proxy[0], proxy[1:])),
    ("bytes", b"abc", lambda proxy: (repr(proxy), len(proxy), proxy[0], proxy[1:])),
    (
        "bytearray",
        bytearray(b"abc"),
        lambda proxy: (repr(proxy), len(proxy), proxy[0], proxy[1:]),
    ),
    (
        "range",
        range(3),
        lambda proxy: (
            repr(proxy),
            len(proxy),
            proxy[1],
            proxy[1:],
            list(reversed(proxy)),
        ),
    ),
]
for label, owner, read in accepted:
    proxy = MappingProxyType(owner)
    print(label, read(proxy))


print("--- bare typing constructor boundary ---")
for label, owner in (
    ("Optional", typing.Optional),
    ("Union", typing.Union),
    ("Callable", typing.Callable),
    ("ClassVar", typing.ClassVar),
    ("Final", typing.Final),
    ("Literal", typing.Literal),
    ("Type", typing.Type),
    ("List", typing.List),
    ("Dict", typing.Dict),
    ("Set", typing.Set),
    ("FrozenSet", typing.FrozenSet),
    ("Tuple", typing.Tuple),
):
    try:
        MappingProxyType(owner)
    except TypeError:
        print(label, "rejected")
    else:
        print(label, "accepted")

try:
    MappingProxyType(typing.Annotated)
except TypeError as error:
    print("Annotated", type(error).__name__, str(error))
else:
    print("Annotated accepted")

for label, owner in (
    ("Generic", typing.Generic),
    ("Protocol", typing.Protocol),
):
    try:
        MappingProxyType(owner)
    except TypeError:
        print(label, "rejected")
    else:
        print(label, "accepted")


def mappingproxy_constructor_result(owner):
    try:
        MappingProxyType(owner)
    except TypeError:
        return "rejected"
    return "accepted"


mutable_typing_list = typing.List
if hasattr(mutable_typing_list, "__pyrust_legacy_alias_of__"):
    del mutable_typing_list.__pyrust_legacy_alias_of__
print("mutated legacy identity", mappingproxy_constructor_result(mutable_typing_list))


old_typing_list = typing.List
old_typing_optional = typing.Optional
old_typing_annotated = typing.Annotated
del sys.modules["typing"]
import typing as reloaded_typing

print(
    "old typing generation",
    mappingproxy_constructor_result(old_typing_list),
    mappingproxy_constructor_result(old_typing_optional),
    mappingproxy_constructor_result(old_typing_annotated),
)
print(
    "new typing generation",
    mappingproxy_constructor_result(reloaded_typing.List),
    mappingproxy_constructor_result(reloaded_typing.Optional),
    mappingproxy_constructor_result(reloaded_typing.Annotated),
)


print("--- absent owner methods stay absent ---")
for method, args in (
    ("keys", ()),
    ("values", ()),
    ("items", ()),
    ("get", ("a",)),
    ("copy", ()),
    ("__reversed__", ()),
):
    try:
        getattr(MappingProxyType("abc"), method)(*args)
    except AttributeError as error:
        print(method, type(error).__name__, str(error))


print("--- exact constructor rejection boundary ---")


class OnlyKeys:
    def keys(self):
        return []


class SomeClass:
    pass


class ListSubclass(list):
    pass


class TupleSubclass(tuple):
    pass


rejected = [
    object(),
    OnlyKeys(),
    [],
    ListSubclass(),
    (),
    TupleSubclass(),
    set(),
    frozenset(),
    deque(),
    DequeSubclass(),
    1,
    None,
    SomeClass,
]
for owner in rejected:
    try:
        MappingProxyType(owner)
        print(type(owner).__name__, "accepted")
    except TypeError as error:
        print(type(owner).__name__, type(error).__name__, str(error))


print("--- live, nested, and read-only ---")
live_owner = UserDict(a=1)
live_proxy = MappingProxyType(live_owner)
live_owner["b"] = 2
print(len(live_proxy), list(live_proxy.items()))
del live_owner["a"]
print(len(live_proxy), list(live_proxy.items()))
nested = MappingProxyType(MappingProxyType(live_owner))
live_owner["c"] = 3
print(len(nested), nested["c"], list(nested))
try:
    nested["blocked"] = 4
except TypeError as error:
    print(type(error).__name__, "item assignment" in str(error))


print("--- exact dict mutation guard ---")
guarded_owner = {"a": 1, "b": 2}
guarded_proxy = MappingProxyType(guarded_owner)
guarded_iter = iter(guarded_proxy)
print(next(guarded_iter))
guarded_owner["c"] = 3
try:
    next(guarded_iter)
except RuntimeError as error:
    print(type(error).__name__, str(error))
