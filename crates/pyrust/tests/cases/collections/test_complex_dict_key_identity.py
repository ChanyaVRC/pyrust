# A complex key with a zero imaginary part keeps its own key object (#2900).
#
# CPython hashes and compares `1`, `1.0`, `True` and `1+0j` as one key, so they
# share a single dict/set slot — but the slot keeps the key object that was
# inserted *first* while the value is overwritten by the last assignment.
# pyrust used to collapse `1+0j` to a float key at insertion, so `{1+0j: 'a'}`
# listed `1.0` and lost the complex type entirely.

# The reported repro: the key round-trips as a complex, not a float.
print(list({1 + 0j: "a"}))
print(list({1 + 0j: "a"}.keys())[0].__class__.__name__)
print(type(list({1 + 0j: 1})[0]) is complex)

# First-inserted key wins; last-assigned value wins.  Every ordering of the
# four numeric types collapses to exactly one entry.
print(list({1: "a", 1 + 0j: "b"}), {1: "a", 1 + 0j: "b"})
print(list({1 + 0j: "a", 1: "b"}), {1 + 0j: "a", 1: "b"})
print(list({1.0: "a", 1 + 0j: "b"}), {1.0: "a", 1 + 0j: "b"})
print(list({1 + 0j: "a", 1.0: "b"}), {1 + 0j: "a", 1.0: "b"})
print(list({True: "a", 1 + 0j: "b"}), {True: "a", 1 + 0j: "b"})
print(list({1 + 0j: "a", True: "b"}), {1 + 0j: "a", True: "b"})
print(list({1 + 0j: "a", 1.0: "b", 1: "c", True: "d"}))
print({1 + 0j: "a", 1.0: "b", 1: "c", True: "d"})

# Zero and its signed/boolean spellings behave the same way.
print(list({False: "x", 0 + 0j: "y"}), list({0 + 0j: "x", False: "y"}))
print(list({0 + 0j: 1}), list({complex(-0.0, 0.0): 1}))
print(list({0: "a", complex(0.0, -0.0): "b"}))

# Cross-type lookup and membership still unify.
print({1 + 0j: "a"}[1], {1 + 0j: "a"}[1.0], {1 + 0j: "a"}[True])
print({1: "a"}[1 + 0j], {1.0: "a"}[1 + 0j])
print((1 + 0j) in {1}, 1 in {1 + 0j}, (1 + 0j) in {1.0})
print(hash(1) == hash(1.0) == hash(1 + 0j), hash(1 + 0j))

# Sets: one slot across the four types, keeping the first element inserted.
print(len({1, 1.0, 1 + 0j, True}))
print(list({1 + 0j, 1, 1.0}), list({1, 1 + 0j}), list({1.0, 1 + 0j}))
print(len({0, 0.0, 0 + 0j, False}))

# A non-zero imaginary part is a distinct key, as before.
print(list({0 + 1j: "a"}), (1 + 2j) in {1 + 2j}, len({1 + 0j, 1 + 1j}))
print(len({1 + 1j: "a", 1: "b"}))

# Large integer-valued reals still unify with int / float across the BigInt arm.
print(list({complex(1e20): "a", 10**20: "b"}))
print(len({complex(1e20): 1, 10**20: 2}), len({10**20: 1, complex(1e20): 2}))
print(list({1e20: "a", complex(1e20): "b"}))

# Fractional reals unify with the equal float.
print(list({0.5: "a", 0.5 + 0j: "b"}), len({0.5, 0.5 + 0j}))

# Infinities unify; a complex NaN key still finds itself by identity.
print(list({float("inf"): "a", complex(float("inf"), 0.0): "b"}))
z = complex(float("nan"), 0.0)
print(z in {z}, {z: 1}[z], len({z: 1, z: 2}))

# frozenset keys and nested tuple keys carry complex elements through unchanged.
print(list({(1 + 0j,): "a"}), (1 + 0j,) in {(1,): "a"})
print(sorted(hash(k) for k in [1, 1.0, 1 + 0j, True]))
