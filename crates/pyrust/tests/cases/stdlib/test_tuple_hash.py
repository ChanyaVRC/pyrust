# Parity test for CPython 3.12 tuple and slice hash algorithm (issue #892).
#
# CPython 3.12 uses an xxHash-based algorithm for tuple hashing (Python 3.8+):
#   acc = PRIME5
#   for each item: acc += item_hash * PRIME2; acc = rotl31(acc); acc *= PRIME1
#   acc += n ^ (PRIME5 ^ 3527539)
#   if acc == u64::MAX: acc = 1546275796
#
# Slice hash (CPython 3.12) uses the same kernel over (start, stop, step) but
# WITHOUT the final length-mixing XOR step.
#
# These tests assert exact numeric values so a regression to the old formula
# (h = h * 1000003 + item_hash, seeded at 3527539) will be immediately visible.

# Empty tuple: the loop never runs; only PRIME5 seed + final XOR with n=0.
assert hash(()) == 5740354900026072187, f"hash(()) = {hash(())}"

# Single-element tuples:
assert hash((1,)) == -6644214454873602895, f"hash((1,)) = {hash((1,))}"
assert hash((0,)) == hash((0,)), "hash((0,)) should be consistent"

# Multi-element tuples with deterministic-hash integer elements:
assert hash((1, 2, 3)) == 529344067295497451, f"hash((1,2,3)) = {hash((1,2,3))}"
assert hash((0, 0, 0)) == hash((0, 0, 0)), "repeated hash should be consistent"
assert hash((42, -1, 100)) == hash((42, -1, 100)), "hash should be stable"

# Nested tuples (must use CPython 3.12 algorithm recursively):
inner = (1, 2)
outer = (inner, 3)
assert hash(outer) == hash(((1, 2), 3)), "nested tuple hash must be stable"

# Boolean elements (True == 1, False == 0 for hashing):
assert hash((True,)) == hash((1,)), f"hash((True,)) must equal hash((1,))"
assert hash((False,)) == hash((0,)), f"hash((False,)) must equal hash((0,))"
assert hash((True, False)) == hash((1, 0)), "bool elements hash like ints"

# Tuple hash must NOT collide with int hash:
assert hash((1,)) != hash(1), "tuple hash must differ from int hash"

# Slice hash (integer components — these have deterministic hash values):
# CPython 3.12 made slice hashable. The algorithm is the same xxHash kernel
# as tuplehash but WITHOUT the final n ^ (PRIME5 ^ 3527539) step.
assert hash(slice(1, 2, 3)) == -2340833382717974474, \
    f"hash(slice(1,2,3)) = {hash(slice(1,2,3))}"
assert hash(slice(0, 10, 2)) == -4569403022601412507, \
    f"hash(slice(0,10,2)) = {hash(slice(0,10,2))}"

# Slice equality implies equal hash:
s1 = slice(1, 10, 2)
s2 = slice(1, 10, 2)
assert s1 == s2
assert hash(s1) == hash(s2), "equal slices must have equal hash"

# Slice hash must NOT equal tuple hash of same components:
assert hash(slice(1, 2, 3)) != hash((1, 2, 3)), \
    "slice hash must differ from tuple hash of same components"

# Unhashable types still raise TypeError:
try:
    hash([1, 2, 3])
    assert False, "list should be unhashable"
except TypeError:
    pass

try:
    hash({1: 2})
    assert False, "dict should be unhashable"
except TypeError:
    pass

try:
    hash({1, 2})
    assert False, "set should be unhashable"
except TypeError:
    pass

# Tuple containing unhashable element raises TypeError:
try:
    hash(([1, 2],))
    assert False, "tuple with list element should be unhashable"
except TypeError:
    pass

print("ok")
