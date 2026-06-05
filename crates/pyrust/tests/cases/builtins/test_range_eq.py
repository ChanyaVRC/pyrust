# Parity fixture for issues #2196 / #2056: range.__eq__ must compare by sequence
# content, not by raw start/stop/step fields.
#
# CPython 3.12 (Objects/rangeobject.c, range_equals): two ranges are equal iff
# they yield the same sequence — same length, and (when non-empty) same first
# element, and (when length >= 2) same step.  This matches the content-based
# range hash, so equal ranges must hash equal.

# ── empty ranges: all empty ranges are equal regardless of start/stop/step ────

print(range(0) == range(2, 2, 7))     # True
print(range(5, 5) == range(0))        # True
print(range(0, 0, -1) == range(0))    # True
print(range(5, 5, 3) == range(2, 2))  # True

# ── length-1 ranges: step is irrelevant ──────────────────────────────────────

print(range(5, 6) == range(5, 6, 99))  # True
print(range(1, 4) == range(1, 4, 1))   # True
print(range(3) == range(0, 3, 1))      # True

# ── length >= 2: different stop, same yielded sequence ────────────────────────

print(range(0, 3, 2) == range(0, 4, 2))    # True  (both -> 0, 2)
print(range(0, 9, 2) == range(0, 10, 2))   # True  (both -> 0,2,4,6,8)
print(range(10, 0, -2) == range(10, 1, -2))  # True  (both -> 10,8,6,4,2)

# ── different sequences stay unequal ──────────────────────────────────────────

print(range(0, 5) == range(0, 6))          # False
print(range(0, 10, 2) == range(0, 11, 2))  # False  (lengths 5 vs 6)
print(range(0, 5) == range(0, 5, 1))       # True

# ── != mirrors == ─────────────────────────────────────────────────────────────

print(range(0, 3, 2) != range(0, 4, 2))  # False
print(range(0, 5) != range(0, 6))        # True

# ── range never equals a list/tuple with the same elements ────────────────────

print(range(1, 4) == [1, 2, 3])    # False
print(range(1, 4) != [1, 2, 3])    # True
print(range(1, 4) == (1, 2, 3))    # False

# ── hash/eq invariant: equal ranges hash equal ────────────────────────────────

print(hash(range(0, 3, 2)) == hash(range(0, 4, 2)))      # True
print(hash(range(0)) == hash(range(5, 5)))               # True
print(hash(range(0)) == hash(range(2, 2)))               # True
print(hash(range(10, 0, -2)) == hash(range(10, 1, -2)))  # True

# ── set/dict dedup of equal ranges ────────────────────────────────────────────

print(len({range(0), range(2, 2), range(5, 5, 3)}))  # 1
print(len({range(0): 'a', range(2, 2): 'b'}))        # 1

# ── big ranges (BigInt-backed bounds) ─────────────────────────────────────────

print(range(10**20, 10**20) == range(0))               # True  (both empty)
print(range(0) == range(10**20, 10**20))               # True  (cross-width)
print(range(0, 10**20, 10**20) == range(0, 1))         # True  (both -> 0)
print(range(0, 1) == range(0, 10**20, 10**20))         # True  (cross-width)
print(range(2 * 10**20, 2 * 10**20) == range(3 * 10**20, 3 * 10**20))  # True
print(range(10**20) == range(0))                       # False
print(range(-(10**20), 0) == range(-(10**20), 0))      # True
print(hash(range(10**20, 10**20)) == hash(range(0)))   # True
print(hash(range(0, 10**20, 10**20)) == hash(range(0, 1)))  # True
print(len({range(10**20, 10**20), range(0)}))          # 1
