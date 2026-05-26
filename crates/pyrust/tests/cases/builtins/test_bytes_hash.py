# Test that bytes objects are hashable (issue #1203).
# bytes is immutable, so it must be usable as dict/set keys and in hash().
#
# We do NOT compare exact hash values: CPython uses SipHash (seeded per-process)
# while pyrust uses FNV-1a, so the numbers will differ.  We check structure and
# identity instead.

# hash() returns an integer
assert isinstance(hash(b"hello"), int)
assert isinstance(hash(b""), int)

# Stability: equal bytes produce the same hash
assert hash(b"hello") == hash(b"hello")
assert hash(b"") == hash(b"")
assert hash(b"\x00\xff") == hash(b"\x00\xff")

# Distinctness (high probability; these are not collisions in either FNV or SipHash)
assert hash(b"hello") != hash(b"world")
assert hash(b"a") != hash(b"b")

# bytes as dict key
d = {b"key": 42}
print(d[b"key"])          # 42
print(b"key" in d)        # True
print(b"other" in d)      # False

# Multiple bytes keys
d2 = {b"a": 1, b"b": 2, b"c": 3}
print(len(d2))            # 3
print(d2[b"a"])           # 1
print(d2[b"b"])           # 2
print(sorted(d2.values())) # [1, 2, 3]

# bytes as set element
s = {b"x", b"y", b"z"}
print(b"x" in s)          # True
print(b"w" in s)          # False
print(len(s))             # 3

# bytes inside a tuple is still hashable
t = (b"hello", b"world")
assert isinstance(hash(t), int)
assert hash(t) == hash(t)

# set deduplication: equal bytes objects collapse to one entry
s2 = {b"dup", b"dup", b"dup"}
print(len(s2))            # 1

# bytes and str are independent types even if content looks the same
d3 = {b"key": "bytes", "key": "str"}
print(len(d3))            # 2  — different keys

# frozenset containing bytes
fs = frozenset({b"a", b"b"})
print(len(fs))            # 2
print(b"a" in fs)         # True
