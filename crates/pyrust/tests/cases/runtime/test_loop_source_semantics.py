# Source-shaped loops must preserve observable Python name, call, and protocol
# semantics; syntax alone never proves canonical range or exact-int behavior.


# A zero-trip for-loop neither binds a new target nor overwrites an existing
# one.  This must also hold for the live module globals dictionary.
module_zero_target = 77
module_globals = globals()
for module_zero_target in range(0):
    raise AssertionError("zero-trip body ran")
assert module_zero_target == 77
assert module_globals["module_zero_target"] == 77

for never_bound_target in range(0):
    raise AssertionError("zero-trip body ran")
assert "never_bound_target" not in globals()


def function_zero_trip():
    target = 88
    for target in range(0):
        raise AssertionError("zero-trip body ran")
    return target


assert function_zero_trip() == 88


# The iterator owns its cursor.  Reassigning or deleting the source-level loop
# target in the body must not corrupt the next iteration.
module_reassigned = []
for module_target in range(4):
    module_reassigned.append(module_target)
    module_target = 100
assert module_reassigned == [0, 1, 2, 3]
assert module_target == 100

module_deleted = []
for deleted_target in range(3):
    module_deleted.append(deleted_target)
    del deleted_target
assert module_deleted == [0, 1, 2]
assert "deleted_target" not in globals()


def function_target_mutation():
    reassigned = []
    for target in range(4):
        reassigned.append(target)
        target = 200

    deleted = []
    for target in range(3):
        deleted.append(target)
        del target
    try:
        target
    except UnboundLocalError:
        target_is_unbound = True
    else:
        target_is_unbound = False
    return reassigned, deleted, target_is_unbound


assert function_target_mutation() == ([0, 1, 2, 3], [0, 1, 2], True)


# The canonical range call must still execute its __index__ conversions.
index_events = []


class RangeBound:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        index_events.append(self.value)
        return self.value


index_values = []
for index_value in range(RangeBound(3)):
    index_values.append(index_value)
assert index_events == [3]
assert index_values == [0, 1, 2]


# A module while condition is evaluated on every test.  Both the argument and
# globals() alias must observe the source-level counter, never an internal
# pre-decremented value.
module_while_i = 0
module_stop_events = []


def module_stop(value):
    module_stop_events.append((value, module_globals["module_while_i"]))
    return 3


while module_while_i < module_stop(module_while_i):
    module_while_i += 1
assert module_stop_events == [(0, 0), (1, 1), (2, 2), (3, 3)]
assert module_globals["module_while_i"] == 3


# A named stop can change in the body; hoisting it once changes the trip count.
dynamic_i = 0
dynamic_stop = 3
dynamic_seen = []
while dynamic_i < dynamic_stop:
    dynamic_seen.append(dynamic_i)
    dynamic_stop -= 1
    dynamic_i += 1
assert dynamic_seen == [0, 1]
assert (dynamic_i, dynamic_stop) == (2, 1)


def function_dynamic_while():
    events = []
    seen = []
    value = 0
    stop_value = 3

    def stop(current):
        events.append(current)
        return stop_value

    while value < stop(value):
        seen.append(value)
        stop_value -= 1
        value += 1
    return events, seen, value, stop_value


assert function_dynamic_while() == ([0, 1, 2], [0, 1], 2, 1)


# The counter shape alone does not prove integer semantics.  User comparison
# and addition methods must run in their source order.
custom_events = []


class Counter:
    def __init__(self, value):
        self.value = value

    def __lt__(self, other):
        custom_events.append(("lt", self.value, other))
        return self.value < other

    def __add__(self, other):
        custom_events.append(("add", self.value, other))
        return Counter(self.value + other)


custom_counter = Counter(0)
custom_seen = []
while custom_counter < 3:
    custom_seen.append(custom_counter.value)
    custom_counter += 1
assert custom_seen == [0, 1, 2]
assert custom_counter.value == 3
assert custom_events == [
    ("lt", 0, 3),
    ("add", 0, 1),
    ("lt", 1, 3),
    ("add", 1, 1),
    ("lt", 2, 3),
    ("add", 2, 1),
    ("lt", 3, 3),
]


# A syntactic name `range` is not proof of canonical builtin identity.
canonical_range = range
shadow_calls = []


def range(argument):
    shadow_calls.append(("module", argument))
    return [42]


module_shadow_values = []
for shadow_value in range(5):
    module_shadow_values.append(shadow_value)
assert module_shadow_values == [42]


def global_shadow_consumer():
    values = []
    for value in range(6):
        values.append(value)
    return values


assert global_shadow_consumer() == [42]


def local_shadow_consumer():
    calls = []

    def range(argument):
        calls.append(argument)
        return [99]

    values = []
    for value in range(7):
        values.append(value)
    return calls, values


assert local_shadow_consumer() == ([7], [99])
assert shadow_calls == [("module", 5), ("module", 6)]
range = canonical_range

print(
    "loop-source-semantics",
    module_zero_target,
    module_reassigned,
    module_deleted,
    module_stop_events,
    dynamic_seen,
    custom_seen,
    module_shadow_values,
)
