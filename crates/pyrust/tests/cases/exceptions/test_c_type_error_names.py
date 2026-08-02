import collections
import decimal
import fractions
import functools
import io
import itertools
import random
import re
import sys
import time
import types


def show(label, operation):
    try:
        operation()
    except (AttributeError, TypeError) as exc:
        print(label + " -> " + type(exc).__name__ + ": " + str(exc))
    else:
        print(label + " -> <no error>")


def show_add(label, value, other=1):
    show(label, lambda: value + other)


# CPython's static stdlib types carry a module-qualified tp_name used only in
# diagnostics. Exercise every affected public type, including the eighteenth
# itertools adapter omitted from the original 28-type issue inventory.
ordered = collections.OrderedDict()
defaulted = collections.defaultdict(int)
deque_value = collections.deque([1])
decimal_value = decimal.Decimal(1)
partial_value = functools.partial(pow, 2)
bytes_io = io.BytesIO()
string_io = io.StringIO()
pattern = re.compile("a")
match = pattern.match("a")
namespace = types.SimpleNamespace()
struct_time = time.struct_time((2020, 6, 15, 1, 2, 3, 0, 167, 0))

itertools_values = [
    ("count", itertools.count()),
    ("cycle", itertools.cycle([])),
    ("repeat", itertools.repeat(None)),
    ("chain", itertools.chain([])),
    ("islice", itertools.islice([], 0)),
    ("zip_longest", itertools.zip_longest([])),
    ("product", itertools.product([])),
    ("accumulate", itertools.accumulate([])),
    ("groupby", itertools.groupby([])),
    ("permutations", itertools.permutations([])),
    ("combinations", itertools.combinations([], 0)),
    (
        "combinations_with_replacement",
        itertools.combinations_with_replacement([], 0),
    ),
    ("starmap", itertools.starmap(pow, [])),
    ("takewhile", itertools.takewhile(bool, [])),
    ("dropwhile", itertools.dropwhile(bool, [])),
    ("filterfalse", itertools.filterfalse(bool, [])),
    ("compress", itertools.compress([], [])),
    ("pairwise", itertools.pairwise([])),
]
grouper = next(itertools.groupby([1]))[1]

show_add("collections.OrderedDict", ordered)
show_add("collections.defaultdict", defaulted)
show_add("decimal.Decimal", decimal_value, "x")
show_add("functools.partial", partial_value)
show_add("_io.BytesIO", bytes_io)
show_add("_io.StringIO", string_io)
show_add("re.Pattern", pattern)
show_add("re.Match", match)
show_add("types.SimpleNamespace", namespace)
for name, value in itertools_values:
    show_add("itertools." + name, value)
show_add("itertools._grouper", grouper)

# Cover all nine diagnostic families that interpolate tp_name.
show("ordering", lambda: deque_value < 1)
show("attribute", lambda: deque_value.missing)
show("call", lambda: itertools.count()())
show("length", lambda: len(match))
show("hash", lambda: hash(ordered))
show("int", lambda: int(struct_time))
show("subscript", lambda: partial_value[0])
show("sequence multiply", lambda: "x" * partial_value)
for symbol, operation in [
    ("&", lambda: ordered & 1),
    ("|", lambda: ordered | 1),
    ("^", lambda: ordered ^ 1),
    ("<<", lambda: ordered << 1),
    (">>", lambda: ordered >> 1),
]:
    show("binary " + symbol, operation)
show("divmod", lambda: divmod(partial_value, 1))
show("pow", lambda: pow(partial_value, 2))
show("delete attribute", lambda: delattr(deque_value, "missing"))


def inplace_set():
    value = {1}
    value &= ordered


def inplace_bytearray():
    value = bytearray(b"x")
    value *= partial_value


show("in-place set", inplace_set)
show("in-place sequence multiply", inplace_bytearray)
show("deque sequence multiply", lambda: deque_value * partial_value)
show("max ordering", lambda: max(partial_value, 1))
show(
    "object delete attribute",
    lambda: object.__delattr__(ordered, "missing"),
)


class NonCallableSlots:
    __len__ = match
    __hash__ = match
    __getitem__ = match


slot_value = NonCallableSlots()
for label, operation in [
    ("length", lambda: len(slot_value)),
    ("hash", lambda: hash(slot_value)),
    ("subscript", lambda: slot_value[0]),
]:
    show("non-callable slot " + label, operation)


def counter_iadd():
    value = collections.Counter(a=1)
    value += partial_value


def counter_iand():
    value = collections.Counter(a=1)
    value &= partial_value


show("Counter attribute adapter", counter_iadd)
show("Counter subscript adapter", counter_iand)

# Python-implemented heap types keep bare diagnostic names even when their
# __module__ is non-builtins.
bare_values = [
    ("Counter", collections.Counter()),
    ("ChainMap", collections.ChainMap()),
    ("UserDict", collections.UserDict()),
    ("UserList", collections.UserList()),
    ("UserString", collections.UserString("x")),
    ("Random", random.Random(1)),
    ("mappingproxy", types.MappingProxyType({})),
]
for name, value in bare_values:
    show("bare " + name, lambda value=value: value - 1)
show("bare Fraction", lambda: fractions.Fraction(1, 2) + "x")

# deque concatenation hardcodes the bare word "deque" in CPython and must not
# start using the diagnostic tp_name facility.
show("deque concat literal", lambda: deque_value + 1)

# The new metadata is error-only: Python-visible names and stable reprs remain
# unchanged. Avoid io repr/type-module checks, which belong to issue #2929.
affected_values = [
    ("OrderedDict", ordered),
    ("defaultdict", defaulted),
    ("deque", deque_value),
    ("Decimal", decimal_value),
    ("partial", partial_value),
    ("BytesIO", bytes_io),
    ("StringIO", string_io),
    ("Pattern", pattern),
    ("Match", match),
    ("SimpleNamespace", namespace),
    ("struct_time", struct_time),
] + itertools_values + [("_grouper", grouper)]
for label, value in affected_values:
    value_type = type(value)
    print("metadata " + label + ":", value_type.__name__, value_type.__qualname__)

print("repr OrderedDict:", repr(collections.OrderedDict([("a", 1)])))
print("repr defaultdict:", repr(collections.defaultdict(None, {"a": 1})))
print("repr deque:", repr(collections.deque([1, 2])))
print("repr Decimal:", repr(decimal.Decimal("1.25")))
print("repr namespace:", repr(types.SimpleNamespace(a=1)))
print("repr struct_time:", repr(struct_time))
print("repr count:", repr(itertools.count(5, 2)))

# Release the struct-sequence instance while its defining type is still fully
# initialized.  Keeping it alive until CPython finalization can make 3.12.3
# clear time.struct_time's type dict first and emit a spurious tp_clear error.
del affected_values
del struct_time


# Metadata belongs only to exact canonical classes. It is neither inherited by
# user subclasses nor inferred from a mutable visible class name.
class OrderedChild(collections.OrderedDict):
    pass


class Lookalike:
    pass


Lookalike.__name__ = "OrderedDict"
show_add("user subclass", OrderedChild())
show_add("visible-name lookalike", Lookalike())


# Re-importable modules mint fresh PyRust class generations. Both retained old
# values and newly imported values must keep their own exact diagnostic tag.
old_ordered = ordered
old_count = itertools.count()
old_grouper = grouper
sys.modules.pop("collections", None)
sys.modules.pop("itertools", None)
import collections as reloaded_collections
import itertools as reloaded_itertools

show_add("reload old collections", old_ordered)
show_add("reload new collections", reloaded_collections.OrderedDict())
show_add("reload old itertools", old_count)
show_add("reload new itertools", reloaded_itertools.count())
show_add("reload old itertools grouper", old_grouper)
show_add(
    "reload new itertools grouper",
    next(reloaded_itertools.groupby([1]))[1],
)
