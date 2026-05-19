# hash(frozenset(...)) must be hashable and match CPython's frozenset_hash
# algorithm (Objects/setobject.c).  Exact numeric values only pinned for
# integer-element frozensets (str hashing is seeded in CPython).

# Basic hashability
print(type(hash(frozenset())))
print(type(hash(frozenset({1}))))
print(type(hash(frozenset({1, 2, 3}))))

# Exact values for integer-element frozensets (deterministic in CPython 3.12+)
print(hash(frozenset()))
print(hash(frozenset({1})))
print(hash(frozenset({1, 2})))
print(hash(frozenset({1, 2, 3})))

# Nested frozenset hash (recursive, deterministic)
print(hash(frozenset({frozenset({1})})))

# Order-independence: same elements -> same hash regardless of insertion order
fs_a = frozenset({1, 2, 3})
fs_b = frozenset({3, 2, 1})
print(hash(fs_a) == hash(fs_b))
print(fs_a == fs_b)

# frozenset as dict key
d = {frozenset({1}): "a", frozenset({2}): "b"}
print(d[frozenset({1})])
print(d[frozenset({2})])

# frozenset membership in dict
print(frozenset({1}) in d)
print(frozenset({3}) in d)

# frozenset as set element
s = {frozenset({1, 2}), frozenset({3})}
print(frozenset({1, 2}) in s)
print(frozenset({3}) in s)
print(frozenset({4}) in s)

# Deduplication: equal frozensets collapse to one entry in a set
print(len({frozenset({1, 2}), frozenset({2, 1})}))

# bool elements hash the same as int (True == 1, False == 0 in Python)
print(hash(frozenset({True})) == hash(frozenset({1})))
print(frozenset({True}) == frozenset({1}))

# mutable set is still unhashable
try:
    hash({1, 2})
except TypeError as e:
    print(e)

# empty frozenset hash is stable
print(hash(frozenset()) == hash(frozenset()))

# float elements: exact values are deterministic (unlike str hashing)
print(hash(frozenset({1.5})))
print(hash(frozenset({0.5, 1.5})))
# float and int with same numeric value hash to the same element hash
print(hash(frozenset({1.0})) == hash(frozenset({1})))
