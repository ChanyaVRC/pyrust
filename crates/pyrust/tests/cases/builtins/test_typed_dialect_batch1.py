# Parity fixtures for the typed-signature dialect migration (#400).
# Covers: any, all, sum, list, tuple, set, frozenset,
#         enumerate, map, filter, reversed, delattr, setattr, round.

# ── any / all ────────────────────────────────────────────────────────────────
assert any([False, True, False]) == True
assert any([False, False]) == False
assert any([]) == False
assert any(range(3)) == True          # truthy when first nonzero seen
assert all([True, True]) == True
assert all([True, False]) == False
assert all([]) == True                # vacuously true

# user-defined iterable
class Three:
    def __iter__(self):
        return iter([1, 0, 1])

assert any(Three()) == True
assert all(Three()) == False

# TypeError for non-iterable
try:
    any(42)
except TypeError:
    print("any-noniter TypeError")

try:
    all(3.14)
except TypeError:
    print("all-noniter TypeError")

# ── sum ──────────────────────────────────────────────────────────────────────
assert sum([1, 2, 3]) == 6
assert sum(range(5)) == 10
assert sum([1, 2], 10) == 13
assert sum([], 99) == 99

# TypeError for non-iterable
try:
    sum(42)
except TypeError:
    print("sum-noniter TypeError")

# ── list / tuple ─────────────────────────────────────────────────────────────
assert list() == []
assert list([1, 2, 3]) == [1, 2, 3]
assert list((4, 5)) == [4, 5]
assert list(range(3)) == [0, 1, 2]
assert tuple() == ()
assert tuple([1, 2]) == (1, 2)
assert tuple(range(2)) == (0, 1)

# user iterable
class Pair:
    def __iter__(self):
        return iter([10, 20])

assert list(Pair()) == [10, 20]
assert tuple(Pair()) == (10, 20)

# ── set / frozenset ──────────────────────────────────────────────────────────
assert set() == set()
assert set([1, 2, 2, 3]) == {1, 2, 3}
assert frozenset() == frozenset()
assert frozenset([1, 1, 2]) == frozenset({1, 2})
# frozenset(frozenset) equality (identity shortcut tracked as follow-up)
fs = frozenset({7, 8})
assert frozenset(fs) == fs

# ── enumerate ────────────────────────────────────────────────────────────────
assert list(enumerate(["a", "b"])) == [(0, "a"), (1, "b")]
assert list(enumerate(["a", "b"], 5)) == [(5, "a"), (6, "b")]
assert list(enumerate(["x"], start=10)) == [(10, "x")]
assert list(enumerate([])) == []

# bool start
assert list(enumerate(["a"], True)) == [(1, "a")]
assert list(enumerate(["a"], False)) == [(0, "a")]

# TypeError for bad start
try:
    list(enumerate(["a"], "bad"))
except TypeError:
    print("enumerate-badstart TypeError")

# ── map ──────────────────────────────────────────────────────────────────────
assert list(map(str, [1, 2, 3])) == ["1", "2", "3"]
assert list(map(lambda x: x * 2, range(4))) == [0, 2, 4, 6]
assert list(map(abs, [-1, 2, -3])) == [1, 2, 3]

# ── filter ───────────────────────────────────────────────────────────────────
assert list(filter(None, [0, 1, False, 2, ""])) == [1, 2]
assert list(filter(lambda x: x > 2, [1, 2, 3, 4])) == [3, 4]
assert list(filter(None, [])) == []

# ── reversed ─────────────────────────────────────────────────────────────────
assert list(reversed([1, 2, 3])) == [3, 2, 1]
assert list(reversed([])) == []
assert list(reversed(range(3))) == [2, 1, 0]

# ── delattr / setattr ────────────────────────────────────────────────────────
class Box:
    pass

b = Box()
b.x = 42
assert b.x == 42
setattr(b, "x", 99)
assert b.x == 99
delattr(b, "x")
try:
    _ = b.x
except AttributeError:
    print("delattr AttributeError")

# setattr / delattr name must be str
try:
    setattr(b, 42, 1)
except TypeError:
    print("setattr-nonstr TypeError")

try:
    delattr(b, 42)
except TypeError:
    print("delattr-nonstr TypeError")

# ── round ────────────────────────────────────────────────────────────────────
assert round(5) == 5
assert round(5, 2) == 5
assert round(True) == 1
assert round(False) == 0
assert round(2.5) == 2          # banker's rounding
assert round(3.5) == 4
assert round(1.2345, 2) == 1.23
assert round(1.2355, 2) == 1.24

# TypeError for non-number
try:
    round("x")
except TypeError:
    print("round-nonnum TypeError")

print("typed-dialect-batch1 OK")
