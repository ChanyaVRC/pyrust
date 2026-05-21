# Parity fixture for issue #916: tuple hash algorithm and slice hash algorithm
# must match CPython 3.12.
#
# CPython 3.12 (Python 3.8+) uses the xxHash-based tuplehash algorithm.
# CPython 3.12 made slices hashable (bpo-109885); their hash is the same
# xxHash accumulation over (start, stop, step) without the length-XOR term.

# Empty tuple
print(hash(()))

# Single-element tuples (int components, no None)
print(hash((1,)))
print(hash((0,)))
print(hash((-1,)))   # hash(-1) == -2 in CPython; tuple wraps it correctly

# Multi-element tuples
print(hash((1, 2, 3)))
print(hash((0, 0, 0)))
print(hash((100, 200, 300)))

# Nested tuples
print(hash(((1, 2), (3, 4))))
print(hash(((1,), (2,), (3,))))

# Slice hashing: all-integer components
print(hash(slice(1, 2, 3)))
print(hash(slice(0, 0, 0)))
print(hash(slice(0, 10, 2)))
print(hash(slice(-5, 5, 1)))

# Tuple containing slice
print(hash((slice(1, 2, 3),)))
print(hash((1, 2, slice(3, 4, 5))))
print(hash((slice(0, 10, 2),)))

# frozenset whose elements are tuples of ints (exercises py_hash_pykey for Tuple)
print(hash(frozenset([(1, 2, 3)])))
print(hash(frozenset([(1, 2, 3), (4, 5, 6)])))

# Unhashable containers inside tuples raise TypeError
try:
    hash(([1, 2],))
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    hash(({1: 2},))
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

# Slice with unhashable component raises TypeError propagating from the component
try:
    hash(slice([1, 2], 3, 4))
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))
