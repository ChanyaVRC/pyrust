# Issue #418 regression matrix — builtins/operations that consume an
# iterable must dispatch user-defined `__iter__` and the legacy
# `__getitem__` sequence-iter protocol through the interpreter's
# `collect_iterable`, not the bare `iter_values` helper that can't
# reach dunders.

# ── Class with __iter__ returning a built-in iterator ──────────────────────
class Seq:
    def __init__(self): self.items = [1, 2, 3]
    def __iter__(self): return iter(self.items)


assert list(enumerate(Seq())) == [(0, 1), (1, 2), (2, 3)]
assert list(map(lambda x: x * 2, Seq())) == [2, 4, 6]
assert list(filter(None, Seq())) == [1, 2, 3]
assert list(filter(lambda x: x > 1, Seq())) == [2, 3]
assert list(zip(Seq(), [10, 20, 30])) == [(1, 10), (2, 20), (3, 30)]
assert list(zip([10, 20, 30], Seq())) == [(10, 1), (20, 2), (30, 3)]
assert sum(Seq()) == 6
assert sum(Seq(), 100) == 106
assert sorted(Seq()) == [1, 2, 3]
assert sorted(Seq(), reverse=True) == [3, 2, 1]
assert min(Seq()) == 1
assert max(Seq()) == 3
assert any(Seq()) is True
assert all(Seq()) is True

# ── Call-site splat: `f(*Seq())` must drain the user iter ─────────────────
def f(*args):
    return args


assert f(*Seq()) == (1, 2, 3)


# ── functools.reduce ───────────────────────────────────────────────────────
from functools import reduce
assert reduce(lambda a, b: a + b, Seq()) == 6
assert reduce(lambda a, b: a + b, Seq(), 100) == 106


# ── itertools.chain ────────────────────────────────────────────────────────
from itertools import chain
assert list(chain(Seq(), [4, 5])) == [1, 2, 3, 4, 5]
assert list(chain([0], Seq())) == [0, 1, 2, 3]


# ── Class implementing __iter__/__next__ (returns self) ────────────────────
class CustomIter:
    def __init__(self, n):
        self.n = n
        self.i = 0
    def __iter__(self):
        return self
    def __next__(self):
        if self.i >= self.n:
            raise StopIteration
        v = self.i
        self.i += 1
        return v


assert list(enumerate(CustomIter(3))) == [(0, 0), (1, 1), (2, 2)]
assert list(map(lambda x: x + 1, CustomIter(3))) == [1, 2, 3]
assert sum(CustomIter(4)) == 6  # 0 + 1 + 2 + 3
assert sorted(CustomIter(3), reverse=True) == [2, 1, 0]
assert max(CustomIter(5)) == 4
assert min(CustomIter(5)) == 0
assert list(zip(CustomIter(3), CustomIter(3))) == [(0, 0), (1, 1), (2, 2)]
assert f(*CustomIter(3)) == (0, 1, 2)


# ── Legacy __getitem__ sequence-iter protocol (#394 / #416) ────────────────
class GetItem:
    def __init__(self): self.items = [10, 20, 30]
    def __getitem__(self, i):
        if i >= len(self.items):
            raise IndexError
        return self.items[i]


assert list(enumerate(GetItem())) == [(0, 10), (1, 20), (2, 30)]
assert sum(GetItem()) == 60
assert sorted(GetItem()) == [10, 20, 30]
assert max(GetItem()) == 30


# ── Generators (already worked, regression guard) ──────────────────────────
def g():
    yield 1
    yield 2
    yield 3


assert list(enumerate(g())) == [(0, 1), (1, 2), (2, 3)]
assert list(map(lambda x: x * 10, g())) == [10, 20, 30]
assert sum(g()) == 6
assert sorted(g()) == [1, 2, 3]


# ── Built-in iterables (regression guard) ──────────────────────────────────
assert list(enumerate([1, 2, 3])) == [(0, 1), (1, 2), (2, 3)]
assert sum([1, 2, 3]) == 6
assert sorted([3, 1, 2]) == [1, 2, 3]
assert list(map(str, [1, 2, 3])) == ["1", "2", "3"]


# ── Container literal splat (list / tuple / set) ───────────────────────────
assert [*Seq()] == [1, 2, 3]
assert (*Seq(),) == (1, 2, 3)
assert {*Seq()} == {1, 2, 3}
assert [*Seq(), *[4, 5]] == [1, 2, 3, 4, 5]
