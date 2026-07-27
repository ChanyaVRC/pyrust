import math


# A bounded stand-in for an unbounded tail: once the first value has been
# produced, touching the unused tail is a hard failure.  prod/fsum/sumprod must
# process that first value before asking for another one.
class GuardedTail:
    def __init__(self, label, first, events):
        self.label = label
        self.first = first
        self.events = events
        self.index = 0

    def __iter__(self):
        self.events.append(self.label + ".iter")
        return self

    def __next__(self):
        self.events.append(self.label + ".next" + str(self.index))
        if self.index == 0:
            self.index = 1
            return self.first
        raise AssertionError(self.label + " tail was consumed")


events = []


class ProdBomb:
    def __mul__(self, other):
        events.append("prod.mul")
        raise RuntimeError("prod first operation")


try:
    math.prod(GuardedTail("prod", 2, events), start=ProdBomb())
except Exception as error:
    print("prod guarded:", type(error).__name__, str(error), events)


events = []


class FloatBomb:
    def __float__(self):
        events.append("fsum.float")
        raise RuntimeError("fsum first conversion")


try:
    math.fsum(GuardedTail("fsum", FloatBomb(), events))
except Exception as error:
    print("fsum guarded:", type(error).__name__, str(error), events)


events = []


class PairBomb:
    def __mul__(self, other):
        events.append("sumprod.mul")
        raise RuntimeError("sumprod first product")


try:
    math.sumprod(
        GuardedTail("p", PairBomb(), events),
        GuardedTail("q", 3, events),
    )
except Exception as error:
    print("sumprod guarded:", type(error).__name__, str(error), events)


# fsum converts each yielded item before requesting the following item.
events = []


class LoggedFloat:
    def __init__(self, value, label):
        self.value = value
        self.label = label

    def __float__(self):
        events.append(self.label + ".float")
        return self.value


class LoggedIterator:
    def __init__(self, label, values, events):
        self.label = label
        self.values = values
        self.events = events
        self.index = 0

    def __iter__(self):
        self.events.append(self.label + ".iter")
        return self

    def __next__(self):
        self.events.append(self.label + ".next" + str(self.index))
        if self.index >= len(self.values):
            raise StopIteration
        value = self.values[self.index]
        self.index += 1
        return value


print(
    "fsum order result:",
    math.fsum(
        LoggedIterator(
            "f",
            [LoggedFloat(1.25, "a"), LoggedFloat(2.75, "b")],
            events,
        )
    ),
)
print("fsum order events:", events)


# sumprod advances p then q once per pair and performs the pair operation
# before advancing either iterator again.
events = []


class LoggedMultiplier:
    def __init__(self, label, value):
        self.label = label
        self.value = value

    def __mul__(self, other):
        events.append(self.label + ".mul")
        return self.value * other


result = math.sumprod(
    LoggedIterator(
        "p",
        [LoggedMultiplier("a", 2), LoggedMultiplier("b", 4)],
        events,
    ),
    LoggedIterator("q", [3, 5], events),
)
print("sumprod lockstep result:", result)
print("sumprod lockstep events:", events)


# A length mismatch probes both iterators at the mismatching position.  A
# non-StopIteration from p, however, propagates without advancing q.
events = []
try:
    math.sumprod(
        LoggedIterator("short", [1], events),
        LoggedIterator("long", [2, 3], events),
    )
except Exception as error:
    print("sumprod mismatch:", type(error).__name__, str(error), events)


events = []


class ErrorIterator:
    def __init__(self, label, events, error):
        self.label = label
        self.events = events
        self.error = error

    def __iter__(self):
        self.events.append(self.label + ".iter")
        return self

    def __next__(self):
        self.events.append(self.label + ".next")
        raise self.error


try:
    math.sumprod(
        ErrorIterator("bad-p", events, RuntimeError("p failed")),
        ErrorIterator("unused-q", events, RuntimeError("q failed")),
    )
except Exception as error:
    print("sumprod p error:", type(error).__name__, str(error), events)


events = []
try:
    math.sumprod(
        LoggedIterator("empty-p", [], events),
        ErrorIterator("bad-q", events, RuntimeError("q failed")),
    )
except Exception as error:
    print("sumprod q error after p stop:", type(error).__name__, str(error), events)


# Exact-list cursors release their element borrow before invoking Python code
# and read the live list again at the following index.
prod_delete_values = []


class DeleteFactor:
    def __rmul__(self, other):
        del prod_delete_values[1]
        return other * 2


prod_delete_values.extend([DeleteFactor(), 5, 7])
print(
    "prod list delete:",
    math.prod(prod_delete_values),
    len(prod_delete_values),
    prod_delete_values[1],
)


fsum_append_values = []


class AppendFloat:
    def __float__(self):
        fsum_append_values.append(2.5)
        return 1.5


fsum_append_values.append(AppendFloat())
print(
    "fsum list append:",
    math.fsum(fsum_append_values),
    len(fsum_append_values),
)


fsum_replace_values = []


class ReplaceFloat:
    def __float__(self):
        fsum_replace_values[1] = 7.0
        return 1.0


fsum_replace_values.extend([ReplaceFloat(), 2.0])
print(
    "fsum list replace:",
    math.fsum(fsum_replace_values),
    fsum_replace_values[1],
)


# Rebinding the name that supplied a list must not replace the source retained
# by the already-created cursor.
prod_rebound_values = []


class RebindFactor:
    def __rmul__(self, other):
        global prod_rebound_values
        prod_rebound_values = [99]
        return other * 2


prod_rebound_values.extend([RebindFactor(), 5])
print(
    "prod list rebind:",
    math.prod(prod_rebound_values),
    prod_rebound_values,
)


# p is fetched before q. Mutating p from q.__next__ therefore affects p's next
# index, not the item already fetched for the current pair.
sumprod_p_values = [2, 99, 3]
sumprod_mutation_events = []


class MutatingQ:
    def __init__(self):
        self.index = 0

    def __iter__(self):
        sumprod_mutation_events.append("q.iter")
        return self

    def __next__(self):
        sumprod_mutation_events.append("q.next" + str(self.index))
        if self.index == 0:
            self.index = 1
            del sumprod_p_values[1]
            return 4
        if self.index == 1:
            self.index = 2
            return 5
        raise StopIteration


print(
    "sumprod list mutation:",
    math.sumprod(sumprod_p_values, MutatingQ()),
    sumprod_p_values,
    sumprod_mutation_events,
)


# p is also advanced before an exact-list q is read.
sumprod_q_values = [10, 20]


class MutatingP:
    def __init__(self):
        self.index = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.index == 0:
            self.index = 1
            sumprod_q_values[0] = 99
            return 1
        if self.index == 1:
            self.index = 2
            return 2
        raise StopIteration


print(
    "sumprod p mutates q:",
    math.sumprod(MutatingP(), sumprod_q_values),
    sumprod_q_values,
)


# Both exact-list element borrows must be gone before multiplication invokes
# Python code. The mutation changes q's next indexed item.
sumprod_q_during_mul = [4, 5, 6]


class DeleteQDuringMul:
    def __mul__(self, other):
        del sumprod_q_during_mul[1]
        return 2 * other


print(
    "sumprod list callback:",
    math.sumprod([DeleteQDuringMul(), 3], sumprod_q_during_mul),
    sumprod_q_during_mul,
)


# Two consumers of the same exact list own independent indexes. Two consumers
# of the same existing iterator instead share that iterator's position.
same_pair_values = [1, 2, 3, 4]
print(
    "sumprod same list:",
    math.sumprod(same_pair_values, same_pair_values),
)

shared_pair_iterator = iter([1, 2, 3, 4])
print(
    "sumprod same iterator:",
    math.sumprod(shared_pair_iterator, shared_pair_iterator),
    list(shared_pair_iterator),
)


# Tuple inputs take the same direct indexed backend.
print(
    "tuple consumers:",
    math.prod((2, 3, 4)),
    math.fsum((1.25, 2.75)),
    math.sumprod((1.0, 2.0), (3.0, 4.0)),
)


# A list subclass is a protocol object, not an exact-list fast-path hit.
subclass_events = []


class IteratingList(list):
    def __iter__(self):
        subclass_events.append("subclass.iter")
        return iter([4, 5])


print(
    "list subclass:",
    math.prod(IteratingList([99])),
    subclass_events,
)


# Passing an existing iterator preserves and advances that iterator's shared
# cursor instead of wrapping or restarting it.
shared_iterator = iter([2, 3, 5])
print(
    "shared iterator:",
    next(shared_iterator),
    math.prod(shared_iterator),
    list(shared_iterator),
)


# Numeric mode transitions are observable at cancellation boundaries. CPython
# first speculates on native-sized exact ints, then on a contiguous run of
# float-containing primitive pairs. Either path stays disabled after its first
# unsupported or overflowing pair.
for p, q in [
    ([1e16, 1, -1e16], [1.0, 1, 1.0]),
    ([1e16, 1.0, -1e16], [1.0, 1, 1.0]),
    ([2, 1e16, 1, -1e16], [1, 1.0, 1, 1.0]),
    ([10**20, 2], [3, 10**19]),
    ([10**20, -1e16, -10], [-10, 1.0, -1e16]),
    ([True, -1, -1.5], [1, 1e16, True]),
]:
    value = math.sumprod(p, q)
    print("sumprod numeric:", repr(value), type(value).__name__)

try:
    math.sumprod([10**400, 1.0], [1, 1])
except Exception as error:
    print("sumprod delayed float overflow:", type(error).__name__, str(error))


# A generic pair ends optimized accumulation permanently. Subsequent primitive
# pairs use ordinary `*` and `+`; optimized modes are not restarted.
events = []


class GenericProduct:
    def __radd__(self, other):
        events.append(("radd", other, type(other).__name__))
        return 40


class GenericMultiplier:
    def __mul__(self, other):
        events.append(("mul", other))
        return GenericProduct()


value = math.sumprod([1.5, GenericMultiplier()], [2.0, 3])
print("sumprod generic end:", value, type(value).__name__, events)

events = []
value = math.sumprod([1.5, GenericMultiplier(), 2.5], [2.0, 3, 2.0])
print("sumprod generic middle:", value, type(value).__name__, events)
