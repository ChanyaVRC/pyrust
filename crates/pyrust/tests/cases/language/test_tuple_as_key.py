# Regression for issue #382: tuples must be usable as dict / set keys.
# `hash((1, 2, 3))` always worked, but `PyKey` had no `Tuple` variant so
# constructing the dict/set entry raised an "unhashable type" error.

# ── Happy path: tuple literal as dict key ────────────────────────────────────
d = {(1, 2): "x"}
print(d[(1, 2)])                 # x
print((1, 2) in d)               # True
print((2, 1) in d)               # False

# Insertion via subscript also works.
d2 = {}
d2[(1, 2)] = "y"
d2[(3, 4)] = "z"
print(d2[(1, 2)])                # y
print(d2[(3, 4)])                # z
print(len(d2))                   # 2

# ── Tuple in set ─────────────────────────────────────────────────────────────
s = {(1, 2), (3, 4), (1, 2)}     # duplicate (1, 2) dedups
print(len(s))                    # 2
print((1, 2) in s)               # True
print((9, 9) in s)               # False

# ── Dict-comprehension over tuple keys ───────────────────────────────────────
grid = {(i, j): i * j for i in range(3) for j in range(3)}
print(grid[(0, 0)])              # 0
print(grid[(1, 2)])              # 2
print(grid[(2, 2)])              # 4
print(len(grid))                 # 9

# ── Nested tuples ────────────────────────────────────────────────────────────
nested = {(1, (2, 3)): "n", ((1, 2), 3): "m"}
print(nested[(1, (2, 3))])       # n
print(nested[((1, 2), 3)])       # m
print(hash((1, (2, 3))) == hash((1, (2, 3))))  # True

# ── Tuples containing frozensets (also hashable) ─────────────────────────────
fs = frozenset([1, 2])
d3 = {(fs, "a"): 1}
print(d3[(frozenset([2, 1]), "a")])   # 1 — frozenset equality is content-based

# ── Empty tuple as key ───────────────────────────────────────────────────────
d4 = {(): "empty"}
print(d4[()])                    # empty

# ── Singleton tuple as key ───────────────────────────────────────────────────
d5 = {(7,): "one"}
print(d5[(7,)])                  # one
print((7,) in d5)                # True
print(7 in d5)                   # False — int 7 ≠ tuple (7,)

# ── Tuples containing unhashable types raise TypeError ───────────────────────
try:
    {([1, 2], 3): "bad"}
    print("FAIL: no TypeError for tuple containing list")
except TypeError as e:
    print("TE:", e)              # TE: unhashable type: 'list'

try:
    {({1: 2}, 3): "bad"}
    print("FAIL: no TypeError for tuple containing dict")
except TypeError as e:
    print("TE:", e)              # TE: unhashable type: 'dict'

try:
    {({1, 2}, 3): "bad"}
    print("FAIL: no TypeError for tuple containing set")
except TypeError as e:
    print("TE:", e)              # TE: unhashable type: 'set'

# Nested unhashable: ((1, [2]), 3) — list buried two levels deep.
try:
    {((1, [2]), 3): "bad"}
    print("FAIL: no TypeError for nested tuple with list")
except TypeError as e:
    print("TE:", e)              # TE: unhashable type: 'list'

# A bare list / dict / set as a dict key — pre-existing behaviour, kept here
# for completeness alongside the tuple case.
try:
    {[1, 2]: "x"}
    print("FAIL")
except TypeError as e:
    print("TE:", e)              # TE: unhashable type: 'list'
