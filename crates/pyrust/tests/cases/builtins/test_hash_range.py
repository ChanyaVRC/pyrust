# Parity fixture for issue #937: range objects must be hashable.
#
# CPython 3.12 range_hash builds a 3-element tuple (length, a, b) and hashes it:
#   len == 0  ->  hash((len, None, None))
#   len == 1  ->  hash((len, start, None))
#   len  > 1  ->  hash((len, start, step))
#
# For len >= 2 the hash value is fully deterministic (no pointer-based
# components), so exact values are compared below.  For len == 0 and len == 1
# the hash includes hash(None) which is pointer-derived and may differ between
# pyrust and CPython; those cases are tested via consistency checks only.

# ── basic hashability ────────────────────────────────────────────────────────

print(type(hash(range(5))))          # <class 'int'>
print(type(hash(range(0))))          # <class 'int'>
print(type(hash(range(1))))          # <class 'int'>
print(type(hash(range(1, 10, 2))))   # <class 'int'>
print(type(hash(range(0, -10, -1)))) # <class 'int'>

# ── exact hash values for len >= 2 (deterministic, pointer-free) ─────────────

print(hash(range(5)))              # deterministic: hash((5, 0, 1))
print(hash(range(2)))              # deterministic: hash((2, 0, 1))
print(hash(range(1, 10, 2)))       # deterministic: hash((5, 1, 2))
print(hash(range(0, -10, -1)))     # deterministic: hash((10, 0, -1))
print(hash(range(-5, 5, 2)))       # deterministic: hash((5, -5, 2))
print(hash(range(100, 200, 7)))    # deterministic: hash((15, 100, 7))

# ── hash consistency for len == 0 and len == 1 ──────────────────────────────
# (pointer-derived component; only test that equal ranges agree)

print(hash(range(0)) == hash(range(0)))       # True
print(hash(range(1)) == hash(range(1)))       # True
print(hash(range(2, 3)) == hash(range(2, 3))) # True

# ── empty ranges: all empty ranges have the same hash ───────────────────────

h_empty = hash(range(0))
print(hash(range(0, 0, 2)) == h_empty)   # True: all empty ranges identical
print(hash(range(5, 5)) == h_empty)      # True
print(hash(range(5, 5, -1)) == h_empty)  # True

# ── single-element ranges: hash depends only on start ────────────────────────
# range(-1, 0) and range(-2, -1) both have start whose hash is -2 (sentinel remap),
# so they must hash the same as each other.

print(hash(range(-1, 0)) == hash(range(-2, -1)))  # True (both start hash to -2)

# ── range as dict key ────────────────────────────────────────────────────────

d = {range(5): 'five', range(0): 'empty', range(1, 10, 2): 'odds'}
print(d[range(5)])          # five
print(d[range(0, 5, 1)])    # five  (same range, different ctor form)
print(d[range(0)])          # empty
print(d[range(1, 10, 2)])   # odds

try:
    d[range(6)]
except KeyError:
    print('KeyError')       # KeyError

# ── range in set ──────────────────────────────────────────────────────────────

s = {range(5), range(0, 5), range(1, 10, 2)}  # range(5) == range(0,5)
print(len(s))               # 2
print(range(5) in s)        # True
print(range(1, 10, 2) in s) # True
print(range(3) in s)        # False

# ── deduplication ─────────────────────────────────────────────────────────────

s2 = {range(5), range(5), range(5)}
print(len(s2))              # 1

# ── hash of multi-element range equals hash of corresponding tuple ────────────
# CPython range_hash for len >= 2 computes hash((len, start, step)), so:

print(hash(range(1, 10, 2)) == hash((5, 1, 2)))   # True (len=5)
print(hash(range(-5, 5, 2)) == hash((5, -5, 2)))   # True (len=5)

# ── frozenset containing a range ──────────────────────────────────────────────

fs = frozenset({range(5), range(0, 5), range(10)})
print(len(fs))              # 2
print(range(5) in fs)       # True
print(range(10) in fs)      # True
print(range(1) in fs)       # False
