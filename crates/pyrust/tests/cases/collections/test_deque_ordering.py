from collections import deque


def compare(left, operation, right):
    if operation == "lt":
        return left < right
    if operation == "le":
        return left <= right
    if operation == "gt":
        return left > right
    return left >= right


def report_error(label, callback):
    try:
        callback()
    except Exception as error:
        print(label, type(error).__name__, str(error))
    else:
        print(label, "NO ERROR")


# Issue #2997: deque ordering is lexicographic for all four operators.
left = deque([1, 2])
right = deque([1, 3])
for operation in ("lt", "le", "gt", "ge"):
    print("basic", operation, compare(left, operation, right))


# The first unequal pair wins; length is consulted only for a proper prefix.
print("unequal before length", deque([1, 2, 3]) < deque([1, 3]))
print("first pair wins", deque([2]) < deque([1, 3]))
print("proper prefix", deque([1, 2]) < deque([1, 2, 0]))
print("proper prefix reverse", deque([1, 2, 0]) < deque([1, 2]))
print("empty prefix", deque([]) < deque([1]))
print("empty equal", deque([]) < deque([]), deque([]) <= deque([]))
print(
    "equal",
    deque([1, 2]) < deque([1, 2]),
    deque([1, 2]) <= deque([1, 2]),
    deque([1, 2]) > deque([1, 2]),
    deque([1, 2]) >= deque([1, 2]),
)
print("maxlen ignored", deque([1, 2], maxlen=2) == deque([1, 2], maxlen=9))
print("maxlen ordering", deque([1, 2], maxlen=2) < deque([1, 3], maxlen=9))


# Equal-prefix scanning uses identity before equality.  In particular, the
# same NaN advances to the next item while a distinct NaN is the deciding pair.
same_nan = float("nan")
other_nan = float("nan")
print("same nan forward", deque([same_nan, 1]) < deque([same_nan, 2]))
print("same nan reverse", deque([same_nan, 2]) < deque([same_nan, 1]))
print("same nan equal", deque([same_nan]) <= deque([same_nan]))
print("same nan strict", deque([same_nan]) < deque([same_nan]))
print("distinct nan", deque([same_nan, 1]) < deque([other_nan, 2]))


class BoomOnEquality:
    def __eq__(self, other):
        raise AssertionError("identity must win")


identical = BoomOnEquality()
print("identity prefix", deque([identical, 1]) < deque([identical, 2]))


# Deque subclasses participate on either side.
class D(deque):
    pass


print("subclass left", D([1, 2]) < deque([1, 3]))
print("subclass right", deque([1, 2]) < D([1, 3]))
print("subclass both", D([1, 2]) <= D([1, 2]))


# Rich comparisons give a proper RHS subtype's swapped operation priority.
# This must happen in the operator router rather than in deque.__lt__ itself:
# directly calling the base dunder does not dispatch the subclass override.
subtype_events = []


class ReflectedD(deque):
    def __gt__(self, other):
        subtype_events.append("gt")
        return False

    def __ge__(self, other):
        subtype_events.append("ge")
        return False

    def __lt__(self, other):
        subtype_events.append("lt")
        return True

    def __le__(self, other):
        subtype_events.append("le")
        return True


for operation in ("lt", "le", "gt", "ge"):
    subtype_events.clear()
    result = compare(deque([1]), operation, ReflectedD([2]))
    print("subtype priority", operation, result, subtype_events)

subtype_events.clear()
print(
    "direct base dunder",
    deque.__lt__(deque([1]), ReflectedD([2])),
    subtype_events,
)


class NotImplementedD(deque):
    def __gt__(self, other):
        subtype_events.append("not implemented")
        return NotImplemented


subtype_events.clear()
print(
    "subtype fallback",
    deque([1]) < NotImplementedD([2]),
    subtype_events,
)


# Each requested element operator is dispatched independently; <=/>/>= are
# not derived by negating <.  Equality is the prefix probe in every case.
events = []


class OrderedElement:
    def __eq__(self, other):
        events.append("eq")
        return False

    def __lt__(self, other):
        events.append("lt")
        return True

    def __le__(self, other):
        events.append("le")
        return False

    def __gt__(self, other):
        events.append("gt")
        return True

    def __ge__(self, other):
        events.append("ge")
        return False


for operation in ("lt", "le", "gt", "ge"):
    events.clear()
    result = compare(deque([OrderedElement()]), operation, deque([object()]))
    print("element dispatch", operation, result, events)


class BaseElement:
    def __eq__(self, other):
        return False

    def __lt__(self, other):
        events.append("base lt")
        return True


class ReflectedElement(BaseElement):
    def __gt__(self, other):
        events.append("subclass gt")
        return False


events.clear()
print(
    "element subtype priority",
    deque([BaseElement()]) < deque([ReflectedElement()]),
    events,
)


# A successful equality probe checks both deque mutation versions before the
# scan continues.  A false final probe follows CPython and lets the requested
# element comparison decide without a post-comparison mutation check.
class MutatingElement:
    def __init__(self, owner, equal):
        self.owner = owner
        self.equal = equal

    def __eq__(self, other):
        self.owner.append("changed")
        return self.equal

    def __lt__(self, other):
        return True


mutated_true = deque()
mutated_true.append(MutatingElement(mutated_true, True))
report_error("mutating true", lambda: mutated_true < deque([object()]))
print("mutating true length", len(mutated_true))

mutated_false = deque()
mutated_false.append(MutatingElement(mutated_false, False))
print("mutating false", mutated_false < deque([object()]), len(mutated_false))


# Non-deque operands remain unsupported and an unorderable element pair must
# surface the element-level error, not a deque-vs-deque error.
for label, callback in (
    ("deque list", lambda: deque([1, 2]) < [1, 3]),
    ("list deque", lambda: [1, 2] < deque([1, 3])),
    ("deque tuple", lambda: deque([1, 2]) < (1, 3)),
    ("deque int", lambda: deque([1]) < 5),
):
    try:
        callback()
    except TypeError:
        print(label, "TypeError")

print(
    "direct nondeque",
    deque.__lt__(deque([1]), [1]) is NotImplemented,
    deque.__ge__(deque([1]), (1,)) is NotImplemented,
)
print("equality nondeque", deque([1, 2]) == [1, 2])

# The raw comparison wrappers still enforce their one-positional-argument
# signature.  Bad call shapes are errors, rather than comparison results.
for operation in ("lt", "le", "gt", "ge"):
    method = getattr(deque([1]), "__" + operation + "__")
    report_error("call shape " + operation + " missing", lambda method=method: method())
    report_error(
        "call shape " + operation + " extra",
        lambda method=method: method(deque([2]), deque([3])),
    )
    report_error(
        "call shape " + operation + " keyword",
        lambda method=method: method(other=deque([2])),
    )
    unbound_method = getattr(deque, "__" + operation + "__")
    report_error(
        "call shape " + operation + " receiver",
        lambda unbound_method=unbound_method: unbound_method(1, deque([2])),
    )
    report_error(
        "call shape " + operation + " no receiver",
        lambda unbound_method=unbound_method: unbound_method(),
    )
    report_error(
        "call shape " + operation + " bad receiver missing",
        lambda unbound_method=unbound_method: unbound_method(1),
    )
    report_error(
        "call shape " + operation + " bad receiver extra",
        lambda unbound_method=unbound_method: unbound_method(
            1, deque([2]), deque([3])
        ),
    )
    report_error(
        "call shape " + operation + " bad receiver keyword",
        lambda unbound_method=unbound_method: unbound_method(1, other=deque([2])),
    )
    report_error(
        "call shape " + operation + " keyword receiver missing",
        lambda unbound_method=unbound_method: unbound_method(other=deque([2])),
    )

report_error("element error", lambda: deque([1]) < deque(["a"]))
