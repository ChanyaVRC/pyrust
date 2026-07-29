"""A released iteration registration is retained, and must stay invisible.

PR #2894 (issue #2890) stopped tearing down a container's
`CollectionMutationState` when its last iterator drops, keeping the
registration resident in a per-thread ring instead.  "Observed" was redefined
from *the registration exists* to *a handle exists*, so a retained registration
must behave exactly like a torn-down one: unrelated writes must not bump it, and
the next walk must restart it as if it had just been created.

Every case below drives a container through a completed walk and then compares
it, step for step, against an identically built container that was never walked.
Sizes straddle the eager frozen-key-order threshold (64), and both the frozen
(int / str keys) and snapshot (tuple keys) representations are covered.
"""

SIZES = (1, 2, 32, 64, 65)


def int_key(index):
    return index * 3


def str_key(index):
    return "k%03d" % index


def tuple_key(index):
    return (index, index)


KEYS = (("int", int_key), ("str", str_key), ("tuple", tuple_key))


def build(size, key):
    return {key(index): index for index in range(size)}


def script(mapping, size, key):
    """A fixed sequence of writes and walks; returns everything observable."""
    steps = [list(mapping), list(mapping.items())]
    mapping[key(size)] = size
    steps.append(list(mapping))
    del mapping[key(0)]
    steps.append(list(mapping))
    mapping[key(0)] = -1
    steps.append(list(mapping))
    mapping.update({key(size + 1): size + 1, key(1): -2})
    steps.append(list(mapping.items()))
    steps.append(list(reversed(list(mapping))))
    steps.append([mapping.popitem() for _ in range(2)])
    steps.append(list(mapping.keys()))
    steps.append(list(mapping.values()))
    mapping.clear()
    steps.append(list(mapping))
    mapping[key(0)] = 0
    steps.append(list(mapping.items()))
    return steps


for name, key in KEYS:
    for size in SIZES:
        fresh = build(size, key)
        used = build(size, key)
        for _ in used:
            pass
        # A second completed walk, so the registration is reused, not rebuilt.
        for _ in used:
            pass
        print(name, size, script(fresh, size, key) == script(used, size, key))

# The retained registration must not make an unrelated write look observed:
# growing a container that nothing is iterating is always legal.
for name, key in KEYS:
    for size in SIZES:
        mapping = build(size, key)
        for _ in mapping:
            pass
        for extra in range(4):
            mapping[key(size + extra)] = extra
        print("grow after walk", name, size, len(mapping), list(mapping)[-1] == key(size + 3))

# ...while growing one that *is* being iterated must still raise, before and
# after the retained registration has been reused.
for name, key in KEYS:
    for size in SIZES:
        mapping = build(size, key)
        results = []
        for attempt in range(2):
            try:
                for _ in mapping:
                    mapping[key(1000 + attempt)] = attempt
            except RuntimeError as error:
                results.append(str(error))
            else:
                results.append("no error")
            del mapping[key(1000 + attempt)]
        print("grow guard", name, size, results[0] == results[1], results[0])

# An abandoned walk releases its handle at a different point than a completed
# one; the next walk must be identical either way.
for name, key in KEYS:
    for size in SIZES:
        broken = build(size, key)
        for _ in broken:
            break
        completed = build(size, key)
        for _ in completed:
            pass
        never = build(size, key)
        print(
            "abandoned",
            name,
            size,
            list(broken) == list(never),
            list(completed) == list(never),
        )

# Nested handles: an outer walk still holds the registration when the inner one
# releases it, so the outer walk must keep seeing a live registration.
for name, key in KEYS:
    for size in (2, 32, 65):
        mapping = build(size, key)
        outer = []
        for outer_key in mapping:
            inner = list(mapping)
            outer.append(inner[0])
            if len(outer) == 2:
                break
        try:
            for outer_key in mapping:
                mapping[key(2000)] = 0
        except RuntimeError as error:
            print("nested", name, size, len(set(outer)) == 1, "RuntimeError")
        else:
            print("nested", name, size, len(set(outer)) == 1, "no error")

# ── Sets take the same registration machinery ────────────────────────────────
def set_script(values, size, key):
    steps = [sorted(values, key=repr), len(values)]
    values.add(key(size))
    steps.append(len(values))
    values.discard(key(0))
    steps.append(sorted(values, key=repr))
    values.update({key(size + 1), key(size + 2)})
    steps.append(sorted(values, key=repr))
    steps.append(sorted(values - {key(1)}, key=repr))
    values.clear()
    steps.append(sorted(values, key=repr))
    return steps


for name, key in KEYS:
    for size in SIZES:
        fresh = {key(index) for index in range(size)}
        used = {key(index) for index in range(size)}
        for _ in used:
            pass
        for _ in used:
            pass
        print("set", name, size, set_script(fresh, size, key) == set_script(used, size, key))

for name, key in KEYS:
    for size in SIZES:
        values = {key(index) for index in range(size)}
        for _ in values:
            pass
        try:
            for _ in values:
                values.add(key(3000))
        except RuntimeError as error:
            print("set grow guard", name, size, str(error))
        else:
            print("set grow guard", name, size, "no error")

# Views built from a walked container must match views from a fresh one.
for name, key in KEYS:
    for size in (1, 64, 65):
        used = build(size, key)
        for _ in used.items():
            pass
        for _ in used.values():
            pass
        for _ in used.keys():
            pass
        fresh = build(size, key)
        print(
            "views",
            name,
            size,
            list(used.keys()) == list(fresh.keys()),
            list(used.values()) == list(fresh.values()),
            list(used.items()) == list(fresh.items()),
            used == fresh,
        )
