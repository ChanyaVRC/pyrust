# frozenset (immutable, hashable set)

# Construction
fs = frozenset([1, 2, 3])
assert len(fs) == 3
assert 2 in fs
assert 4 not in fs

empty = frozenset()
assert len(empty) == 0
assert not empty
assert fs

# From string (each char becomes an element)
chars = frozenset("hello")
assert "h" in chars
assert "x" not in chars

# isinstance
assert isinstance(fs, frozenset)
assert not isinstance(fs, set)
assert not isinstance({1, 2}, frozenset)
assert isinstance({1, 2}, set)

# Iteration
total = 0
for x in frozenset([1, 2, 3, 4]):
    total += x
assert total == 10

# Equality (content-based)
assert frozenset([1, 2, 3]) == frozenset([3, 2, 1])
assert frozenset([1, 2]) != frozenset([1, 2, 3])

# Frozenset vs set equality (CPython: equal across types)
assert frozenset([1, 2, 3]) == {1, 2, 3}
assert {1, 2} == frozenset([1, 2])

# Hashability: can be used as dict key
d = {frozenset([1, 2]): "a", frozenset([3, 4]): "b"}
assert d[frozenset([1, 2])] == "a"
assert d[frozenset([2, 1])] == "a"   # content-based key

# Can be an element of a set
ss = {frozenset([1, 2]), frozenset([3, 4])}
assert frozenset([1, 2]) in ss
assert frozenset([5, 6]) not in ss

# Set operators on frozensets
a = frozenset([1, 2, 3])
b = frozenset([3, 4, 5])
assert a | b == frozenset([1, 2, 3, 4, 5])
assert a & b == frozenset([3])
assert a - b == frozenset([1, 2])
assert a ^ b == frozenset([1, 2, 4, 5])

# Mixed: set OP frozenset → frozenset (per CPython)
mixed = a | {6, 7}
assert isinstance(mixed, frozenset)

# Methods
assert a.union(b) == frozenset([1, 2, 3, 4, 5])
assert a.intersection(b) == frozenset([3])
assert a.difference(b) == frozenset([1, 2])
assert a.symmetric_difference(b) == frozenset([1, 2, 4, 5])
assert a.issubset(frozenset([1, 2, 3, 4]))
assert frozenset([1, 2]).issubset(a)
assert a.issuperset(frozenset([1, 2]))
assert a.isdisjoint(frozenset([4, 5]))
assert not a.isdisjoint(frozenset([3, 4]))

# copy returns frozenset (since source is frozenset)
c = a.copy()
assert c == a
assert isinstance(c, frozenset)

# frozenset(frozenset) is idempotent
fs2 = frozenset(fs)
assert fs2 == fs

# repr
assert repr(frozenset()) == "frozenset()"

# repr preserves CPython's whole-number-float trailing `.0` (issue #422).
# The fixture predates the consolidation onto pyrust_core::key_repr; before
# the fix, frozenset's local copy used `f64::to_string()` and printed `1`.
assert repr(frozenset([1.0])) == "frozenset({1.0})"
assert repr(frozenset([frozenset([1.0])])) == "frozenset({frozenset({1.0})})"

print("frozenset OK")
