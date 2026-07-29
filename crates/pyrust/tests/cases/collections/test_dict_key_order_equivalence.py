"""Key identity must survive the frozen key-order representation.

PR #2894 (issue #2890) lets a small dict/set walk hold its order as the values
it yields rather than as a `PyKey` snapshot.  That round-trip is only sound if
the yielded value reproduces the *stored* key exactly, so every place where
Python's key identity is coarser than object identity is a hazard:
`0 == 0.0 == -0.0 == False`, `1 == 1.0 == True`, and NaN, which is unequal to
itself but still findable by identity.

Tuple keys cannot be reproduced from their yielded value, so they stay on the
general snapshot walk; the two representations must be indistinguishable,
including while the container is mutated mid-walk.
"""

# ── The zero family collapses to whichever key was stored first ──────────────
zero = {}
for key, value in ((0, "int"), (-0.0, "negzero"), (False, "bool"), (0.0, "float")):
    zero[key] = value
    print("zero step", [repr(k) for k in zero], list(zero.values()))
print("zero final", len(zero), [type(k).__name__ for k in zero], zero[0], zero[-0.0])

negzero_first = {-0.0: "negzero"}
negzero_first[0] = "int"
negzero_first[False] = "bool"
print("negzero first", [repr(k) for k in negzero_first], list(negzero_first.values()))

bool_first = {False: "bool", True: "bool"}
bool_first[0] = "int"
bool_first[1] = "int"
print("bool first", [repr(k) for k in bool_first], list(bool_first.values()))

one = {1: "int"}
one[1.0] = "float"
one[True] = "bool"
print("one", [repr(k) for k in one], list(one.values()), one[1], one[1.0], one[True])

# The same collapse in a set, and through `dict.fromkeys`.
print("zero set", [repr(v) for v in {0, -0.0, False, 0.0}])
print("one set", [repr(v) for v in {1, 1.0, True}])
print("fromkeys", [repr(k) for k in dict.fromkeys([0, -0.0, False, 1, True, 1.0])])

# `pop` / `get` / `in` all agree with the stored key.
zero = {-0.0: "stored"}
print("zero lookups", 0 in zero, False in zero, 0.0 in zero, zero.get(0), zero.pop(False))

# A wide int and its float twin are the same key only when the float is exact.
wide = {2**53: "int"}
wide[float(2**53)] = "float"
print("2**53", len(wide), [repr(k) for k in wide], list(wide.values()))

wide = {2**53 + 1: "int"}
wide[float(2**53)] = "float"
print("2**53+1", len(wide), sorted(repr(k) for k in wide))

huge = {2**70: "big"}
print("bigint float", float(2**70) in huge, huge[2**70])


# ── NaN survives the round trip and is still found by its own object ─────────
# Only same-object lookups are pinned here: whether a *distinct* NaN object is a
# separate key is a PyKey-equality question that pyrust and CPython currently
# answer differently, and it is not what the frozen key order decides.
nan = float("nan")
nan_map = {nan: "first", 1: "one", "s": "str"}
print("nan keys", [repr(k) for k in nan_map], nan_map[nan])
print("nan lookups", nan in nan_map, len(nan_map))

nan_map[nan] = "second"
print("nan rewrite", len(nan_map), nan_map[nan], [repr(k) for k in nan_map])
del nan_map[nan]
print("after nan delete", len(nan_map), nan in nan_map, [repr(k) for k in nan_map])

nan_set = {nan, 1, 2}
nan_set.add(nan)
print("nan set", len(nan_set), nan in nan_set, sorted(nan_set - {nan}))

# NaN inside a compound key is reachable through the same tuple contents.
nan_tuple = {(nan, 1): "a", (nan, 2): "b"}
print("nan tuple", len(nan_tuple), (nan, 1) in nan_tuple, nan_tuple[(nan, 2)])


# ── The frozen order must yield the stored key, not the query key ────────────
def walk_types(label, pairs):
    mapping = {}
    for key, value in pairs:
        mapping[key] = value
    print(label, [(type(k).__name__, repr(k)) for k in mapping], list(mapping.values()))


walk_types("stored int", ((1, "a"), (1.0, "b"), (True, "c")))
walk_types("stored float", ((1.0, "a"), (1, "b"), (True, "c")))
walk_types("stored bool", ((True, "a"), (1, "b"), (1.0, "c")))
walk_types("stored mixed", ((0, "a"), (1.0, "b"), (2, "c"), (-0.0, "d"), (True, "e")))

# Repeated walks of the same mapping must be stable.
stable = {0: "a", 1.5: "b", True: "c", "s": "d", b"t": "e", None: "f"}
print("stable", [list(stable) == [k for k in stable] for _ in range(3)])
print("stable repr", [repr(k) for k in stable])
print("stable reversed", [repr(k) for k in reversed(stable)])


# ── Tuple keys stay on the general walk, including mid-mutation ──────────────
def tuple_map(size):
    return {(index, index * index): index for index in range(size)}


for size in (1, 2, 8, 65):
    mapping = tuple_map(size)
    iterator = iter(mapping)
    prefix = [next(iterator)]
    # A value-only update never resizes, so the walk continues.
    mapping[(0, 0)] = -1
    suffix = list(iterator)
    print("tuple value update", size, prefix, len(suffix), prefix[0] not in suffix)

for size in (2, 8, 65):
    mapping = tuple_map(size)
    iterator = iter(mapping)
    prefix = [next(iterator)]
    mapping[(size, size)] = size
    try:
        rest = list(iterator)
    except RuntimeError as error:
        print("tuple growth", size, prefix, str(error))
    else:
        print("tuple growth", size, prefix, "no error", len(rest))

# A mapping whose keys are a mix of frozen-eligible and tuple keys picks the
# general walk for the whole container.
mixed = {1: "int", (2,): "tuple", "three": "str", 4.0: "float"}
print("mixed keys", [repr(k) for k in mixed], list(mixed.values()))
iterator = iter(mixed)
first = next(iterator)
mixed[1] = "updated"
print("mixed update", repr(first), [repr(k) for k in iterator])

# Nested tuples and tuples holding the collapsing scalars.
nested = {((0,), 1): "a", ((0.0,), 1.0): "b", ((False,), True): "c"}
print("nested tuple", len(nested), list(nested.values()), nested[((0,), 1)])

frozen = {frozenset({0, 1}): "a", frozenset({False, True}): "b", frozenset({0.0}): "c"}
print("frozenset", len(frozen), sorted(frozen.values()))

# Emptying and refilling a tuple-key mapping between walks.
mapping = tuple_map(8)
for _ in mapping:
    pass
mapping.clear()
print("cleared", list(mapping))
mapping.update(tuple_map(3))
print("refilled", list(mapping))
