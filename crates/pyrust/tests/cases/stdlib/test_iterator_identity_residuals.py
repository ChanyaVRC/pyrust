# Concrete iterator identity, deque mutation hints, and copy boundaries (#2934).

import copy
import collections
import functools
import itertools
import operator
import os
import pathlib
from collections import Counter, defaultdict, deque


def stable_repr(value):
    # Iterator reprs contain the process-local object address.  Keep the
    # concrete public type prefix while removing only that unstable suffix.
    return repr(value).split(" at 0x")[0] + ">"


def show_type(label, value):
    cls = type(value)
    print(label, str(cls), cls.__module__, cls.__name__, stable_repr(value))


def show_next(label, value):
    try:
        print(label, "value", next(value))
    except Exception as error:
        print(label, "error", type(error).__name__, str(error))


def show_materialized(label, value):
    try:
        print(label, "value", list(value))
    except Exception as error:
        print(label, "error", type(error).__name__)


def show_copy_error(label, value):
    try:
        copy.copy(value)
        print(label, "copied")
    except Exception as error:
        print(label, type(error).__name__, str(error))


def show_constructor(label, call):
    try:
        value = call()
        print(label, "created", type(value).__module__, type(value).__name__)
    except Exception as error:
        # The concrete iterator allocators use different CPython C helpers,
        # whose wording is not part of this boundary.  They must all refuse an
        # invalid construction with TypeError and never return a bare object.
        print(label, type(error).__name__)


def show_protocol(label, call):
    try:
        print(label, "value", call())
    except Exception as error:
        print(label, "error", type(error).__name__)


def show_deque_constructor(label, cls, *args):
    try:
        iterator = cls(*args)
        print(label, "value", operator.length_hint(iterator), list(iterator))
    except Exception as error:
        print(label, "error", type(error).__name__)


def show_deque_reduce_index(label, cls):
    try:
        iterator = cls(deque([10, 20, 30]), 1)
        before = iterator.__reduce__()[1][1]
        value = next(iterator)
        after = iterator.__reduce__()[1][1]
        print(label, "value", before, value, after)
    except Exception as error:
        print(label, "error", type(error).__name__)


print("--- concrete types ---")
show_type("bytearray", iter(bytearray(b"ab")))
show_type("deque", iter(deque([1, 2])))
show_type("deque-reverse", reversed(deque([1, 2])))

print(
    "collections-deque-iterator-export",
    hasattr(collections, "_deque_iterator"),
    getattr(collections, "_deque_iterator", None) is type(iter(deque())),
    "_deque_iterator" in getattr(collections, "__all__", ()),
    hasattr(collections, "_deque_reverse_iterator"),
)

for label, iterator in (
    ("bytearray", iter(bytearray(b"ab"))),
    ("deque", iter(deque([1, 2]))),
    ("deque-reverse", reversed(deque([1, 2]))),
):
    print(
        label + "-nested-repr",
        repr([iterator]) == "[" + repr(iterator) + "]",
        repr((iterator,)) == "(" + repr(iterator) + ",)",
        repr({"it": iterator}) == "{'it': " + repr(iterator) + "}",
    )

ordinary_generator = (value for value in [1])
print(
    "generator-nested-repr",
    repr([ordinary_generator]) == "[" + repr(ordinary_generator) + "]",
)

for label, iterator in (
    ("bytearray", iter(bytearray(b"ab"))),
    ("deque", iter(deque([1, 2]))),
    ("deque-reverse", reversed(deque([1, 2]))),
):
    expected = repr(iterator)
    print(
        label + "-repr-conversions",
        ascii(iterator) == expected,
        "%r" % iterator == expected,
        "{!r}".format(iterator) == expected,
        f"{iterator!r}" == expected,
    )


print("--- concrete type surface ---")


def attribute_mutation_outcome(make, name, delete):
    iterator = make()
    try:
        if delete:
            delattr(iterator, name)
        else:
            setattr(iterator, name, "changed")
        return "value"
    except Exception as error:
        return type(error).__name__


def object_arity_outcome(iterator, method, expected, route):
    try:
        values = [None] * (expected + 1)
        if route == "bound":
            getattr(iterator, method)(*values)
        elif route == "type":
            getattr(type(iterator), method)(iterator, *values)
        else:
            getattr(object, method)(iterator, *values)
        return "value"
    except Exception as error:
        return type(error).__name__


def format_outcome(iterator, spec, route):
    try:
        if route == "bound":
            result = iterator.__format__(spec)
        elif route == "type":
            result = type(iterator).__format__(iterator, spec)
        else:
            result = object.__format__(iterator, spec)
        return "value", result == repr(iterator)
    except Exception as error:
        return type(error).__name__, str(error)


def object_getstate_is_none_or_unavailable(value):
    try:
        return object.__getstate__(value) is None
    except AttributeError:
        # PyRust has not globally exposed object.__getstate__ yet; normalise
        # that pre-existing gap while guarding against provider-state leaks.
        return True


def identity_key_outcome(iterator):
    try:
        mapping = {iterator: "value"}
        values = {iterator}
        return mapping[iterator], iterator in values
    except Exception as error:
        return type(error).__name__, False


class IteratorCollisionProbe:
    def __init__(self, key_hash, events):
        self.key_hash = key_hash
        self.events = events

    def __hash__(self):
        return self.key_hash

    def __eq__(self, other):
        self.events.append(type(other).__name__)
        return True


def reflected_identity_key_outcome(iterator):
    events = []
    probe = IteratorCollisionProbe(hash(iterator), events)
    mapping = {iterator: "value"}
    values = {iterator}
    return mapping.get(probe), probe in values, events


for label, make in (
    ("bytearray", lambda: iter(bytearray(b"ab"))),
    ("deque", lambda: iter(deque([1, 2]))),
    ("deque-reverse", lambda: reversed(deque([1, 2]))),
):
    iterator = make()
    cls = type(iterator)
    owned = (
        "__new__",
        "__getattribute__",
        "__iter__",
        "__next__",
        "__length_hint__",
        "__reduce__",
        "__doc__",
    )
    print(label + "-owned", [name for name in owned if name in cls.__dict__])
    getattribute_outcomes = []
    for call in (
        lambda: cls.__getattribute__(object(), "__class__"),
        lambda: cls.__getattribute__(iterator),
        lambda: cls.__getattribute__(iterator, "__class__", None),
    ):
        try:
            call()
            getattribute_outcomes.append("value")
        except Exception as error:
            getattribute_outcomes.append(type(error).__name__)
    print(
        label + "-getattribute-owned",
        "__module__" in cls.__dict__,
        cls.__dict__["__getattribute__"] is object.__getattribute__,
        cls.__getattribute__(iterator, "__class__") is cls,
        iterator.__getattribute__("__class__") is cls,
        iterator.__getattribute__("__length_hint__")() == operator.length_hint(iterator),
        getattribute_outcomes,
    )
    names = dir(iterator)
    print(
        label + "-carrier-surface",
        hasattr(iterator, "send"),
        hasattr(iterator, "throw"),
        hasattr(iterator, "close"),
        "__reduce__" in names,
        "__reduce_ex__" in names,
    )
    inherited = (
        "__sizeof__",
        "__dir__",
        "__repr__",
        "__str__",
        "__hash__",
        "__eq__",
        "__format__",
        "__getattribute__",
    )
    print(
        label + "-object-surface",
        all(hasattr(iterator, name) for name in inherited),
        all(name in names for name in inherited),
        isinstance(iterator.__sizeof__(), int),
        isinstance(iterator.__repr__(), str),
        isinstance(iterator.__hash__(), int),
        isinstance(cls.__hash__(iterator), int),
        isinstance(object.__hash__(iterator), int),
        isinstance(hash(iterator), int),
    )
    print(
        label + "-display",
        str(iterator) == repr(iterator),
        f"{iterator}" == repr(iterator),
        tuple(format_outcome(iterator, "", route) for route in ("bound", "type", "object")),
        tuple(format_outcome(iterator, "x", route) for route in ("bound", "type", "object")),
    )
    show_protocol(
        label + "-protocol",
        lambda cls=cls, iterator=iterator: (
            cls.__iter__(iterator) is iterator,
            cls.__length_hint__(iterator),
            cls.__next__(iterator),
            len(cls.__reduce__(iterator)),
        ),
    )
    show_constructor(label + "-empty", lambda cls=cls: cls())
    show_constructor(label + "-new", lambda cls=cls: cls.__new__(cls))
    print(
        label + "-attribute-mutation",
        *(
            attribute_mutation_outcome(make, name, delete)
            for name in ("__name__", "gi_running")
            for delete in (False, True)
        ),
    )
    object_arity = []
    for method, expected in (
        ("__sizeof__", 0),
        ("__dir__", 0),
        ("__repr__", 0),
        ("__str__", 0),
        ("__hash__", 0),
        ("__eq__", 1),
        ("__ne__", 1),
        ("__lt__", 1),
        ("__le__", 1),
        ("__gt__", 1),
        ("__ge__", 1),
        ("__format__", 1),
        ("__getattribute__", 1),
    ):
        routes = ("bound", "type", "object") if hasattr(cls, method) else ("bound", "object")
        object_arity.append(
            (method, tuple(object_arity_outcome(make(), method, expected, route) for route in routes))
        )
    print(label + "-object-arity", object_arity)
    print(label + "-identity-key", identity_key_outcome(make()))
    print(label + "-reflected-identity-key", reflected_identity_key_outcome(make()))
    identity = make()
    distinct = make()
    print(
        label + "-object-identity",
        identity == identity,
        identity != identity,
        identity == distinct,
        identity != distinct,
        identity.__eq__(identity),
        identity.__ne__(identity),
        identity.__repr__() == repr(identity),
        object.__repr__(identity) == repr(identity),
    )


grouped = itertools.groupby([1, 1, 2])
next(grouped)
print(
    "global-getstate-provider-boundary",
    object_getstate_is_none_or_unavailable(deque([1], maxlen=3)),
    object_getstate_is_none_or_unavailable(defaultdict(list, {"a": 1})),
    object_getstate_is_none_or_unavailable(functools.cmp_to_key(lambda a, b: 0)(1)),
    object_getstate_is_none_or_unavailable(grouped),
)


# Unlike bytearray_iterator, the deque iterator constructors accept the live
# deque they will walk; keep that valid path beside the invalid-call controls.
forward_type = type(iter(deque()))
reverse_type = type(reversed(deque()))
show_deque_constructor("deque-construct", forward_type, deque([3, 4]))
show_deque_constructor("deque-reverse-construct", reverse_type, deque([3, 4]))


class IndexOne:
    def __index__(self):
        return 1


class BrokenIndex:
    def __index__(self):
        raise LookupError("index boom")


def bytearray_setstate(label, state):
    iterator = iter(bytearray(b"abc"))
    try:
        result = iterator.__setstate__(state)
        print(label, "value", result, operator.length_hint(iterator), list(iterator))
    except Exception as error:
        print(label, "error", type(error).__name__)


print("bytearray-setstate-owned", "__setstate__" in type(iter(bytearray())).__dict__)
unbound_bytearray = iter(bytearray(b"abc"))
print(
    "bytearray-setstate-unbound",
    type(unbound_bytearray).__setstate__(unbound_bytearray, 1),
    list(unbound_bytearray),
)
for state_label, state in (
    ("negative", -2),
    ("one", 1),
    ("beyond", 99),
    ("bool", True),
    ("huge-positive", 10**100),
    ("huge-negative", -(10**100)),
    ("index", IndexOne()),
    ("float", 1.0),
):
    bytearray_setstate("bytearray-setstate-" + state_label, state)

spent_bytearray = iter(bytearray(b"a"))
next(spent_bytearray)
show_next("bytearray-setstate-exhaust-next", spent_bytearray)
spent_reduce = spent_bytearray.__reduce__()
print(
    "bytearray-setstate-exhausted",
    spent_bytearray.__setstate__(1),
    spent_bytearray.__reduce__() == spent_reduce,
    list(spent_bytearray),
)


def show_reduce_ex(label, iterator, protocol):
    try:
        reduced = iterator.__reduce_ex__(protocol)
        print(label, "value", len(reduced), reduced == iterator.__reduce__())
    except Exception as error:
        print(label, "error", type(error).__name__)


def reduce_ex_outcome(iterator, protocol, route):
    try:
        if route == "bound":
            iterator.__reduce_ex__(protocol)
        elif route == "type":
            type(iterator).__reduce_ex__(iterator, protocol)
        else:
            object.__reduce_ex__(iterator, protocol)
        return "value"
    except Exception as error:
        return type(error).__name__


for label, cls in (("deque", forward_type), ("deque-reverse", reverse_type)):
    for index_label, index in (
        ("one", 1),
        ("negative", -10),
        ("end", 3),
        ("beyond", 99),
        ("bool", True),
        ("index", IndexOne()),
    ):
        show_deque_constructor(
            label + "-" + index_label,
            cls,
            deque([10, 20, 30]),
            index,
        )

    show_deque_reduce_index(label + "-reduce-index", cls)

    show_deque_constructor(label + "-zero-args", cls)
    show_deque_constructor(label + "-three-args", cls, deque(), 0, 0)
    show_deque_constructor(label + "-wrong-source", cls, [])
    show_deque_constructor(label + "-float-index", cls, deque(), 1.5)
    show_deque_constructor(label + "-broken-index", cls, deque(), BrokenIndex())


for label, make in (
    ("bytearray", lambda: iter(bytearray(b"ab"))),
    ("deque", lambda: iter(deque([1, 2]))),
    ("deque-reverse", lambda: reversed(deque([1, 2]))),
):
    show_reduce_ex(label + "-reduce-ex-int", make(), 4)
    show_reduce_ex(label + "-reduce-ex-index", make(), IndexOne())
    show_reduce_ex(label + "-reduce-ex-bad", make(), "4")
    width_outcomes = [
        reduce_ex_outcome(make(), protocol, "bound")
        for protocol in (-(2**31), 2**31 - 1, -(2**31) - 1, 2**31)
    ]
    width_outcomes.extend(
        reduce_ex_outcome(make(), protocol, route)
        for protocol in (-(10**100), 10**100)
        for route in ("bound", "type", "object")
    )
    print(label + "-reduce-ex-width", *width_outcomes)
    iterator = make()
    try:
        reduced = type(iterator).__reduce_ex__(iterator, 4)
        print(
            label + "-reduce-ex-unbound",
            "value",
            len(reduced),
            reduced == iterator.__reduce__(),
        )
    except Exception as error:
        print(label + "-reduce-ex-unbound", "error", type(error).__name__)
    show_protocol(
        label + "-object-reduce",
        lambda iterator=iterator: object.__reduce__(iterator),
    )


print("--- deque creation quota and mutation latch ---")

plain = iter(deque([1, 2, 3]))
plain_counts = [operator.length_hint(plain)]
for _ in range(3):
    next(plain)
    plain_counts.append(operator.length_hint(plain))
print("plain-countdown", plain_counts)

data = deque([1, 2])
iterator = iter(data)
data.append(3)
print("grow-no-next", operator.length_hint(iterator))

data = deque([1, 2, 3])
iterator = iter(data)
data.pop()
print("shrink-no-next", operator.length_hint(iterator))

for label, cls in (("forward-end", forward_type), ("reverse-end", reverse_type)):
    data = deque([1, 2])
    iterator = cls(data, len(data))
    data.append(3)
    show_next(label + "-mutated-next", iterator)

for label, cls in (("forward-end", forward_type), ("reverse-end", reverse_type)):
    data = deque([1, 2])
    iterator = cls(data, len(data))
    data.append(3)
    show_materialized(label + "-mutated-list", iterator)

for label, make_iterator in (("forward-spent", iter), ("reverse-spent", reversed)):
    data = deque([1, 2])
    iterator = make_iterator(data)
    next(iterator)
    next(iterator)
    data.append(3)
    show_materialized(label + "-mutated-list", iterator)

data = deque([1, 2])
iterator = iter(data)
data.append(3)
print("grow-before-latch", operator.length_hint(iterator))
show_next("grow-next", iterator)
print("grow-after-latch", operator.length_hint(iterator))
show_next("grow-next-again", iterator)

data = deque([1, 2])
iterator = iter(data)
data.rotate(1)
print("rotate-before-latch", operator.length_hint(iterator))
show_next("rotate-next", iterator)
print("rotate-after-latch", operator.length_hint(iterator))

data = deque([1])
iterator = reversed(data)
data.append(2)
print("reverse-grow-before-latch", operator.length_hint(iterator))
show_next("reverse-grow-next", iterator)
print("reverse-grow-after-latch", operator.length_hint(iterator))
show_next("reverse-grow-next-again", iterator)
print("reverse-order", list(reversed(deque([1, 2, 3]))))

for label, make_iterator in (("deque", iter), ("deque-reverse", reversed)):
    data = deque([0, 1, 2, 3])
    iterator = make_iterator(data)
    next(iterator)
    next(iterator)
    data.clear()
    print(
        label + "-shrink-before-latch-reduce",
        iterator.__reduce__()[1][1],
        [
            copy.copy(iterator).__reduce__()[1][1],
            copy.deepcopy(iterator).__reduce__()[1][1],
        ],
    )


print("--- exhausted bytes-like copy ---")
for label, make in (
    ("bytes", lambda: iter(b"ab")),
    ("bytearray", lambda: iter(bytearray(b"ab"))),
):
    iterator = make()
    live_copy = copy.copy(iterator)
    print(
        label + "-live",
        type(live_copy).__module__,
        type(live_copy).__name__,
        list(live_copy),
    )
    list(iterator)
    spent_copy = copy.copy(iterator)
    print(
        label + "-spent",
        type(spent_copy).__module__,
        type(spent_copy).__name__,
        list(spent_copy),
    )


print("--- copy policy boundaries ---")
show_copy_error("environ", iter(os.environ))
show_copy_error("generator", (item for item in [1]))

# Other standard-library APIs backed by an owned native frame also expose a
# real generator in CPython and must use the same refusal policy.
path = pathlib.Path(".")
show_copy_error("path-iterdir", path.iterdir())
show_copy_error("path-glob", path.glob("*"))

# A normal native sequence cursor remains reducible and independent.
native = iter([1, 2])
next(native)
native_copy = copy.copy(native)
print("native-copy", list(native_copy), list(native))

# A provider-owned cursor remains in the provider path; the environment frame
# policy must not be inferred from the coarse ValueKind::Generator carrier.
provider = Counter("aab").elements()
next(provider)
provider_copy = copy.copy(provider)
print("provider-copy", list(provider_copy), list(provider))

# A deque iterator reduces through its owning deque.  Shallow copy keeps that
# exact owner; deep copy replaces both the deque and its nested elements while
# preserving the remaining values and direction.
class BytearrayCarrier(bytearray):
    pass


bytearray_source = BytearrayCarrier(b"abc")
bytearray_source.visible = [1]
bytearray_iterator = iter(bytearray_source)
bytearray_descriptor_iterator = bytearray.__iter__(bytearray_source)
next(bytearray_iterator)
bytearray_shallow = copy.copy(bytearray_iterator)
bytearray_deep = copy.deepcopy(bytearray_iterator)
bytearray_shallow_source = bytearray_shallow.__reduce__()[1][0]
bytearray_deep_source = bytearray_deep.__reduce__()[1][0]
print(
    "bytearray-subclass-copy-source",
    bytearray_descriptor_iterator.__reduce__()[1][0] is bytearray_source,
    bytearray_iterator.__reduce__()[1][0] is bytearray_source,
    bytearray_shallow_source is bytearray_source,
    type(bytearray_shallow_source) is BytearrayCarrier,
    bytearray_deep_source is bytearray_source,
    type(bytearray_deep_source) is BytearrayCarrier,
    bytearray_shallow_source.visible is bytearray_source.visible,
    bytearray_deep_source.visible == bytearray_source.visible,
    bytearray_deep_source.visible is bytearray_source.visible,
    list(bytearray_shallow),
    list(bytearray_deep),
)


class ListReplacingBytearray(bytearray):
    def __deepcopy__(self, memo):
        ListReplacingBytearray.replacement = [9, 8, 7, 6]
        return ListReplacingBytearray.replacement


class BaseReplacingBytearray(bytearray):
    def __deepcopy__(self, memo):
        return bytearray([9, 8, 7, 6])


class InvalidReplacingBytearray(bytearray):
    def __deepcopy__(self, memo):
        return 42


class IterOverrideBytearray(bytearray):
    def __iter__(self):
        return iter([9, 8])


override_iterator = bytearray.__iter__(IterOverrideBytearray(b"abc"))
next(override_iterator)
for copier in (copy.copy, copy.deepcopy):
    copied = copier(override_iterator)
    print(
        "bytearray-iter-override-" + copier.__name__,
        type(copied).__name__,
        operator.length_hint(copied),
        list(copied),
    )


class ShapedIterOverrideBytearray(bytearray):
    shape = ""

    def __iter__(self):
        return {
            "str-ascii": lambda: iter("abc"),
            "str-unicode": lambda: iter("aé𝄞b"),
            "list": lambda: iter([0, 1, 2, 3]),
            "tuple": lambda: iter((0, 1, 2, 3)),
            "range": lambda: iter(range(4)),
            "big-range": lambda: iter(range(10**20)),
            "bytes": lambda: iter(b"abcd"),
            "reverse": lambda: reversed([0, 1, 2, 3]),
            "custom": EmptyCustomIterator,
            "generator": lambda: (value for value in [0, 1, 2, 3]),
        }[self.shape]()


class EmptyCustomIterator:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration


for shape in (
    "str-ascii",
    "str-unicode",
    "list",
    "tuple",
    "range",
    "big-range",
    "bytes",
    "reverse",
    "custom",
    "generator",
):
    source = ShapedIterOverrideBytearray(b"xyz")
    source.shape = shape
    iterator = bytearray.__iter__(source)
    next(iterator)
    next(iterator)
    outcomes = []
    for copier in (copy.copy, copy.deepcopy):
        try:
            copied = copier(iterator)
            remaining = (
                [next(copied), next(copied)] if shape == "big-range" else list(copied)
            )
            outcomes.append((type(copied).__name__, remaining))
        except Exception as error:
            if shape in ("custom", "generator"):
                outcomes.append(
                    (
                        type(error).__name__,
                        "__dict__" in str(error),
                        "__setstate__" in str(error),
                    )
                )
            else:
                outcomes.append((type(error).__name__,))
    print("bytearray-iter-shape-" + shape, outcomes)


class PreadvancedReverseBytearray(bytearray):
    def __iter__(self):
        replacement = reversed([0, 1, 2, 3])
        next(replacement)
        next(replacement)
        return replacement


preadvanced_reverse = bytearray.__iter__(PreadvancedReverseBytearray(b"abcd"))
next(preadvanced_reverse)
next(preadvanced_reverse)
next(preadvanced_reverse)
print(
    "bytearray-iter-preadvanced-reverse",
    [list(copier(preadvanced_reverse)) for copier in (copy.copy, copy.deepcopy)],
)


class EnvironIterOverrideBytearray(bytearray):
    def __iter__(self):
        return iter(os.environ)


environ_iterator = bytearray.__iter__(EnvironIterOverrideBytearray(b"abc"))
next(environ_iterator)
for copier in (copy.copy, copy.deepcopy):
    try:
        copier(environ_iterator)
        print("bytearray-environ-override-" + copier.__name__, "value")
    except Exception as error:
        print(
            "bytearray-environ-override-" + copier.__name__,
            type(error).__name__,
            "__dict__" in str(error),
            "__setstate__" in str(error),
        )


list_replacement_iterator = iter(ListReplacingBytearray(b"abc"))
next(list_replacement_iterator)
list_replacement_copy = copy.deepcopy(list_replacement_iterator)
list_replacement_hint = operator.length_hint(list_replacement_copy)
list_replacement_remaining = list(copy.copy(list_replacement_copy))
ListReplacingBytearray.replacement.append(5)
print(
    "bytearray-replacement-list",
    type(list_replacement_copy).__name__,
    type(ListReplacingBytearray.replacement).__name__,
    4 - list_replacement_hint,
    list_replacement_remaining,
    list(list_replacement_copy),
)

base_replacement_iterator = iter(BaseReplacingBytearray(b"abc"))
next(base_replacement_iterator)
base_replacement_copy = copy.deepcopy(base_replacement_iterator)
base_replacement_reduce = base_replacement_copy.__reduce__()
print(
    "bytearray-replacement-bytearray",
    type(base_replacement_copy).__name__,
    type(base_replacement_reduce[1][0]).__name__,
    base_replacement_reduce[2],
    list(base_replacement_copy),
)

invalid_iterator = iter(InvalidReplacingBytearray(b"abc"))
next(invalid_iterator)
try:
    copy.deepcopy(invalid_iterator)
    print("bytearray-replacement-invalid", "value")
except Exception as error:
    print("bytearray-replacement-invalid", type(error).__name__)


class ShortReplacingBytearray(bytearray):
    def __deepcopy__(self, memo):
        return bytearray(b"x")


short_iterator = iter(ShortReplacingBytearray(b"abcd"))
next(short_iterator)
next(short_iterator)
short_copy = copy.deepcopy(short_iterator)
short_source = short_copy.__reduce__()[1][0]
short_source.extend(b"y")
print(
    "bytearray-replacement-clamp",
    short_copy.__reduce__()[2],
    next(short_copy, "stop"),
)


class CyclicBytearray(bytearray):
    pass


cyclic_source = CyclicBytearray(b"abc")
cyclic_iterator = iter(cyclic_source)
cyclic_source.iterator = cyclic_iterator
cyclic_copy = copy.deepcopy(cyclic_iterator)
cyclic_copy_source = cyclic_copy.__reduce__()[1][0]
cyclic_nested = cyclic_copy_source.iterator
print(
    "bytearray-copy-cycle",
    cyclic_copy is cyclic_nested,
    cyclic_nested.__reduce__()[1][0] is cyclic_copy_source,
    list(cyclic_copy),
    list(cyclic_nested),
)


class ResizedDeepcopyDeque(deque):
    def __deepcopy__(self, memo):
        return deque([9, 8, 7, 6])


class InvalidDeepcopyDeque(deque):
    def __deepcopy__(self, memo):
        return [9, 8, 7, 6]


for label, make in (
    ("deque", lambda value: iter(value)),
    ("deque-reverse", lambda value: reversed(value)),
):
    source = deque([[1], [2], [3]])
    iterator = make(source)
    next(iterator)
    shallow = copy.copy(iterator)
    deep = copy.deepcopy(iterator)
    shallow_source = shallow.__reduce__()[1][0]
    deep_source = deep.__reduce__()[1][0]
    print(
        label + "-copy-source",
        shallow_source is source,
        deep_source is source,
        shallow_source[1] is source[1],
        deep_source[1] is source[1],
        list(shallow),
        list(deep),
    )

    mutated_source = deque([1, 2, 3])
    mutated = make(mutated_source)
    next(mutated)
    mutated_source.append(4)

    def copy_states():
        states = []
        for copier in (copy.copy, copy.deepcopy):
            copied = copier(mutated)
            states.append(
                (
                    operator.length_hint(copied),
                    copied.__reduce__()[1][1],
                    list(copied),
                )
            )
        return states

    print(label + "-mutated-copy-before", copy_states())
    show_next(label + "-mutated-next", mutated)
    print(label + "-mutated-copy-after", copy_states())

    resized = make(ResizedDeepcopyDeque([1, 2, 3]))
    next(resized)
    resized_copy = copy.deepcopy(resized)
    print(
        label + "-resized-deepcopy",
        resized_copy.__reduce__()[1][1],
        operator.length_hint(resized_copy),
        list(resized_copy),
    )
    invalid = make(InvalidDeepcopyDeque([1, 2, 3]))
    next(invalid)
    try:
        copy.deepcopy(invalid)
        print(label + "-invalid-deepcopy", "value")
    except Exception as error:
        print(label + "-invalid-deepcopy", type(error).__name__)


print("--- operator __all__ green control ---")
expected_operator_all = [
    "abs",
    "add",
    "and_",
    "attrgetter",
    "call",
    "concat",
    "contains",
    "countOf",
    "delitem",
    "eq",
    "floordiv",
    "ge",
    "getitem",
    "gt",
    "iadd",
    "iand",
    "iconcat",
    "ifloordiv",
    "ilshift",
    "imatmul",
    "imod",
    "imul",
    "index",
    "indexOf",
    "inv",
    "invert",
    "ior",
    "ipow",
    "irshift",
    "is_",
    "is_not",
    "isub",
    "itemgetter",
    "itruediv",
    "ixor",
    "le",
    "length_hint",
    "lshift",
    "lt",
    "matmul",
    "methodcaller",
    "mod",
    "mul",
    "ne",
    "neg",
    "not_",
    "or_",
    "pos",
    "pow",
    "rshift",
    "setitem",
    "sub",
    "truediv",
    "truth",
    "xor",
]
print("operator-all", operator.__all__ == expected_operator_all, len(operator.__all__))
