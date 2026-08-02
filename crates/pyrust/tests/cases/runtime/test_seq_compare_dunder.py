# Issue #2817: list/tuple ordering must dispatch rich comparisons on the first
# element pair that is not equal, rather than using only the primitive Rust
# comparator.


class Number:
    def __init__(self, value):
        self.value = value

    def __eq__(self, other):
        return isinstance(other, Number) and self.value == other.value

    def __lt__(self, other):
        return self.value < other.value

    def __le__(self, other):
        return self.value <= other.value

    def __gt__(self, other):
        return self.value > other.value

    def __ge__(self, other):
        return self.value >= other.value


print("tuple ops", (Number(1),) < (Number(2),), (Number(1),) <= (Number(1),))
print("tuple ops", (Number(2),) > (Number(1),), (Number(2),) >= (Number(2),))
print("list ops", [Number(1)] < [Number(2)], [Number(1)] <= [Number(1)])
print("list ops", [Number(2)] > [Number(1)], [Number(2)] >= [Number(2)])

# Equality controls prefix scanning; ordering applies only to the first
# unequal pair, and a wholly equal shorter sequence is ordered by length.
print(
    "equal prefix",
    (Number(1), Number(2)) < (Number(1), Number(3)),
    [Number(1), Number(2)] > [Number(1), Number(0)],
)
print(
    "proper prefix",
    (Number(1),) < (Number(1), Number(2)),
    [Number(1), Number(2)] >= [Number(1)],
)


# The exact requested operation is used on the differing pair.  CPython
# returns the element comparison result directly, including non-bools.
class Exact:
    def __eq__(self, other):
        return False

    def __lt__(self, other):
        return "lt-result"

    def __le__(self, other):
        return "le-result"

    def __gt__(self, other):
        return "gt-result"

    def __ge__(self, other):
        return "ge-result"


print("exact lt", [Exact()] < [Exact()])
print("exact le", (Exact(),) <= (Exact(),))
print("exact gt", [Exact()] > [Exact()])
print("exact ge", (Exact(),) >= (Exact(),))


# NotImplemented falls through to the reflected comparison.
events = []


class DecliningLeft:
    def __eq__(self, other):
        return False

    def __lt__(self, other):
        events.append("left lt")
        return NotImplemented


class ReflectedRight:
    def __eq__(self, other):
        return False

    def __gt__(self, other):
        events.append("right gt")
        return "reflected-result"


print("reflected", [DecliningLeft()] < [ReflectedRight()], events)


# A proper RHS subtype receives the swapped comparison first.
class Base:
    def __eq__(self, other):
        return False

    def __lt__(self, other):
        events.append("base lt")
        return "base-result"


class Sub(Base):
    def __gt__(self, other):
        events.append("sub gt")
        return "sub-result"


events.clear()
print("subtype", (Base(),) < (Sub(),), events)


# If both ordering slots decline, the original operator-specific TypeError is
# preserved rather than manufacturing an ordering.
class Declines:
    def __eq__(self, other):
        return False

    def __lt__(self, other):
        return NotImplemented

    def __gt__(self, other):
        return NotImplemented


try:
    [Declines()] < [Declines()]
    print("declines no error")
except TypeError:
    print("declines TypeError")


# Equality and ordering exceptions propagate out of the container comparison.
class EqBoom:
    def __eq__(self, other):
        raise ValueError("eq boom")


class LtBoom:
    def __eq__(self, other):
        return False

    def __lt__(self, other):
        raise RuntimeError("lt boom")


for label, operation in (
    ("eq exception", lambda: [EqBoom()] < [EqBoom()]),
    ("lt exception", lambda: (LtBoom(),) < (LtBoom(),)),
):
    try:
        operation()
        print(label, "no error")
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))


# Nested concrete sequences recurse through the same interpreter-aware path.
print("nested list", [[Number(1)]] < [[Number(2)]])
print("nested tuple", [(Number(2),)] > [(Number(1),)])


class TruthyEqual:
    def __eq__(self, other):
        return "equal"


print("truthy equality", [TruthyEqual()] < [TruthyEqual(), 1])


class ListSubclass(list):
    pass


print("list subclass", ListSubclass([Number(1)]) < ListSubclass([Number(2)]))

# Sequence comparison re-reads live list lengths after an equal element pair.
# In particular, an equality hook may mutate either operand before the proper
# prefix result is chosen.
mutation_left = []
mutation_right = []


class AppendOnEqual:
    def __eq__(self, other):
        mutation_left.append(0)
        return True


mutation_left.append(AppendOnEqual())
mutation_right.append(AppendOnEqual())
print(
    "mutation length",
    mutation_left > mutation_right,
    len(mutation_left),
    len(mutation_right),
)

# If equality replaces the current pair before returning false, ordering uses
# the live replacement pair rather than the objects that were tested for
# equality.
replacement_events = []
replacement_left = []
replacement_right = []


class ReplaceOnUnequal:
    def __eq__(self, other):
        replacement_events.append("eq")
        replacement_left[0] = 0
        replacement_right[0] = 10
        return False

    def __lt__(self, other):
        replacement_events.append("old lt")
        return "old-result"


replacement_left.append(ReplaceOnUnequal())
replacement_right.append(ReplaceOnUnequal())
print(
    "mutation replacement",
    replacement_left < replacement_right,
    replacement_events,
    replacement_left,
    replacement_right,
)

# If equality removes the current index, there is no differing pair left to
# order.  The four operators therefore use the live sequence lengths.
removal_events = []
removal_left = []
removal_right = []


class RemoveOnUnequal:
    def __eq__(self, other):
        removal_events.append("eq")
        removal_left.clear()
        removal_right.clear()
        return False

    def __lt__(self, other):
        removal_events.append("old lt")
        return "lt-result"

    def __le__(self, other):
        removal_events.append("old le")
        return "le-result"

    def __gt__(self, other):
        removal_events.append("old gt")
        return "gt-result"

    def __ge__(self, other):
        removal_events.append("old ge")
        return "ge-result"


def removal_case(operation):
    removal_events.clear()
    removal_left.append(RemoveOnUnequal())
    removal_right.append(RemoveOnUnequal())
    if operation == "lt":
        result = removal_left < removal_right
    elif operation == "le":
        result = removal_left <= removal_right
    elif operation == "gt":
        result = removal_left > removal_right
    else:
        result = removal_left >= removal_right
    return result, list(removal_events)


print(
    "mutation removal",
    removal_case("lt"),
    removal_case("le"),
    removal_case("gt"),
    removal_case("ge"),
)

# Identity-prefix, primitives, and mixed sequence types retain their existing
# behavior and stay on the primitive fast path where possible.
same = Number(1)
print("identity", [same] <= [same], (same,) < (same,))
print("primitive", [1, 2] < [1, 3], (2, 0) >= (1, 9))
try:
    [Number(1)] < (Number(2),)
    print("mixed no error")
except TypeError:
    print("mixed TypeError")

# Sequence-element identity is checked before recursive ordering, which also
# terminates self-referential containers without a native stack overflow.
cycle = []
cycle.append(cycle)
print("self cycle", cycle < cycle, cycle <= cycle, cycle > cycle, cycle >= cycle)
print("shared cycle", [cycle] < [cycle], [cycle] <= [cycle])

# Reusing the same distinct child pair in two sibling positions is a DAG, not
# a cycle.  The active-pair guard must leave the first child before visiting
# the second one, including in optimized builds where debug assertions vanish.
dag_left_child = [0]
dag_right_child = [0]
try:
    print(
        "shared dag",
        [dag_left_child, dag_left_child] < [dag_right_child, dag_right_child],
        [dag_left_child, dag_left_child] <= [dag_right_child, dag_right_child],
    )
except RecursionError:
    print("shared dag RecursionError")

distinct_cycle_left = []
distinct_cycle_right = []
distinct_cycle_left.append(distinct_cycle_left)
distinct_cycle_right.append(distinct_cycle_right)
for label, operation in (
    ("lt", lambda: distinct_cycle_left < distinct_cycle_right),
    ("le", lambda: distinct_cycle_left <= distinct_cycle_right),
    ("gt", lambda: distinct_cycle_left > distinct_cycle_right),
    ("ge", lambda: distinct_cycle_left >= distinct_cycle_right),
):
    try:
        operation()
        print("distinct cycle", label, "no error")
    except RecursionError:
        print("distinct cycle", label, "RecursionError")

# The interpreter-free comparator is also used by ordering reducers.  A
# recursive pair must propagate RecursionError there instead of being retried
# through the user-comparison fallback and treated as an equal prefix.
for label, operation in (
    ("min", lambda: min(distinct_cycle_left, distinct_cycle_right)),
    ("max", lambda: max(distinct_cycle_left, distinct_cycle_right)),
    ("min iterable", lambda: min([distinct_cycle_left, distinct_cycle_right])),
    ("max iterable", lambda: max([distinct_cycle_left, distinct_cycle_right])),
    ("sorted", lambda: sorted([distinct_cycle_left, distinct_cycle_right])),
):
    try:
        operation()
        print("distinct cycle reducer", label, "no error")
    except RecursionError:
        print("distinct cycle reducer", label, "RecursionError")


def nested_list(depth, leaf):
    result = leaf
    for _ in range(depth):
        result = [result]
    return result


# CPython's C-level sequence comparison is not limited by the Python call
# recursion setting.  Deep, finite acyclic inputs therefore remain orderable.
try:
    print("deep finite", nested_list(1100, 0) < nested_list(1100, 1))
except RecursionError:
    print("deep finite RecursionError")
