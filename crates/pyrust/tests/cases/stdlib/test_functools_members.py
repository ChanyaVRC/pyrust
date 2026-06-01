# functools.cache, functools.cmp_to_key, functools.total_ordering — issue #1876.

import functools

# --- functools.cache: unbounded memo (== lru_cache(maxsize=None)) ---
calls = []


@functools.cache
def square(n):
    calls.append(n)
    return n * n


print("cache", square(3), square(3), square(4), square(3))
print("cache-calls", calls)  # each distinct arg computed once
print("cache-has-clear", hasattr(square, "cache_clear"))
square.cache_clear()
print("cache-after-clear", square(3))
print("cache-calls2", calls)  # recomputed after clear
print("cache-not-none", functools.cache is not None)

# --- functools.cmp_to_key: legacy comparator → key function ---
print("ascending", sorted([3, 1, 2], key=functools.cmp_to_key(lambda a, b: a - b)))
print("descending", sorted([3, 1, 2], key=functools.cmp_to_key(lambda a, b: b - a)))
print(
    "by-length",
    sorted(["aaa", "b", "cc"], key=functools.cmp_to_key(lambda a, b: len(a) - len(b))),
)
print("min", min([3, 1, 2], key=functools.cmp_to_key(lambda a, b: a - b)))
print("max", max([3, 1, 2], key=functools.cmp_to_key(lambda a, b: a - b)))
# Stability: equal comparator results preserve input order.
pairs = [(1, "a"), (1, "b"), (0, "c"), (1, "d")]
print("stable", sorted(pairs, key=functools.cmp_to_key(lambda x, y: x[0] - y[0])))
# Direct comparison + key objects are unhashable (CPython's functools.K).
_k = functools.cmp_to_key(lambda a, b: a - b)
print("direct-cmp", _k(1) < _k(2), _k(2) <= _k(1), _k(1) != _k(2))
try:
    hash(_k(1))
    print("hashable")
except TypeError:
    print("unhashable-key")


# --- functools.total_ordering: derive missing ordering ops ---
@functools.total_ordering
class FromLt:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return self.v == o.v

    def __lt__(self, o):
        return self.v < o.v


@functools.total_ordering
class FromGt:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return self.v == o.v

    def __gt__(self, o):
        return self.v > o.v


@functools.total_ordering
class FromLe:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return self.v == o.v

    def __le__(self, o):
        return self.v <= o.v


@functools.total_ordering
class FromGe:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return self.v == o.v

    def __ge__(self, o):
        return self.v >= o.v


for cls in (FromLt, FromGt, FromLe, FromGe):
    for a in range(3):
        for b in range(3):
            x, y = cls(a), cls(b)
            print(cls.__name__, a, b, x < y, x <= y, x > y, x >= y, x == y, x != y)

# total_ordering must not clobber an already-defined ordering op.
@functools.total_ordering
class KeepsGt:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return self.v == o.v

    def __lt__(self, o):
        return self.v < o.v

    def __gt__(self, o):
        return "custom-gt"


print("keeps-custom", KeepsGt(1) > KeepsGt(2))
print("derived-le", KeepsGt(1) <= KeepsGt(2))

# No ordering op → ValueError (assert the message text CPython uses).
try:

    @functools.total_ordering
    class NoOrdering:
        def __eq__(self, o):
            return True

except ValueError as e:
    print("value-error", str(e))
