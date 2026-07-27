# Hashable aggregates inside an iterable operand must force the interpreter-
# aware set-algebra path when they transitively contain a user object.  The
# observable __hash__/__eq__ direction and mutating-method representative
# selection below are CPython 3.12 behavior.

events = []


class K:
    def __init__(self, label, value=1, boom=False):
        self.label = label
        self.value = value
        self.boom = boom

    def __hash__(self):
        events.append("h:" + self.label)
        return self.value

    def __eq__(self, other):
        events.append("e:" + self.label + ":" + other.label)
        if self.boom:
            raise RuntimeError("boom:" + self.label)
        return self.value == other.value


def tuple_labels(values):
    return [item[0].label for item in values]


def method_case(method):
    a = K("a")
    b = K("b")
    target = {(a,)}
    events.clear()
    result = getattr(target, method)([(b,)])
    if result is None:
        result = target
    return tuple_labels(result), events.copy()


for name in (
    "union",
    "intersection",
    "difference",
    "symmetric_difference",
    "update",
    "intersection_update",
    "difference_update",
    "symmetric_difference_update",
):
    print(name, method_case(name))

a = K("a")
b = K("b")
target = {(a,)}
events.clear()
print("isdisjoint", target.isdisjoint([(b,)]), events)

# frozenset and slice are immutable/hashable aggregate keys too.  They cannot
# form aggregate cycles, so the runtime predicate can recurse without a
# visited-set; mutable nested containers remain on the unhashable error path.
a = K("fa")
b = K("fb")
left = frozenset({a})
right = frozenset({b})
events.clear()
result = set().union([left, right])
print("nested-frozenset", len(result), events)

a = K("sa")
b = K("sb")
left = slice(a, None, None)
right = slice(b, None, None)
events.clear()
result = set().union([left, right])
print("nested-slice", len(result), events)

primitive_slice = slice(1, 5, 2)
primitive_result = set().union([primitive_slice])
print("primitive-slice", len(primitive_result), primitive_slice in primitive_result)

mutable_cycle = []
mutable_cycle.append(mutable_cycle)
try:
    set().union(mutable_cycle)
except TypeError as error:
    print("mutable-cycle", str(error))


def error_case(method, incoming_boom=False):
    a = K("a", boom=not incoming_boom)
    b = K("b", boom=incoming_boom)
    target = {(a,)}
    events.clear()
    try:
        result = getattr(target, method)([(b,)])
        if result is None:
            result = target
        return "no-error", tuple_labels(result), events.copy()
    except RuntimeError as error:
        return str(error), tuple_labels(target), events.copy()


for name in (
    "union",
    "intersection",
    "difference",
    "isdisjoint",
    "update",
    "intersection_update",
    "difference_update",
    "symmetric_difference_update",
):
    print("error-" + name, error_case(name))

# The non-mutating symmetric-difference algorithm starts from the incoming
# set, so that incoming representative owns the comparison.
print(
    "error-symmetric_difference",
    error_case("symmetric_difference", incoming_boom=True),
)
