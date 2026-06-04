# collections.defaultdict two-argument constructor — issue #2099.
#
# CPython: `defaultdict(default_factory=None, /, *args, **kwargs)`.  The first
# positional is the factory; the remaining positional (a mapping or an
# iterable of (key, value) pairs) plus any keyword arguments initialise the
# dict exactly like `dict(...)`.  Bad arguments raise `TypeError` (a
# non-callable, non-None factory) or the standard dict-update errors.
#
# The parity harness diffs byte-for-byte against CPython 3.12.
#
# Reference: https://docs.python.org/3/library/collections.html#collections.defaultdict

import collections
from collections import defaultdict, Counter

# ── factory + mapping ────────────────────────────────────────────────────
print(defaultdict(int, {"a": 1, "b": 2}))
print(defaultdict(list, {"x": [1]}))

# ── factory + iterable of pairs ──────────────────────────────────────────
print(defaultdict(int, [("x", 1), ("y", 2)]))
print(defaultdict(int, (("a", 1),)))
# Two-char strings are length-2 sequences → (key, value) pairs.
print(defaultdict(int, ["ab", "cd"]))

# ── factory + keyword args (+ optional positional) ───────────────────────
print(defaultdict(int, a=1, b=2))
print(defaultdict(int, {"a": 1}, b=2))
print(defaultdict(int, [("a", 1)], b=2))

# ── factory + another mapping (dict-subclass / duck-typed) ───────────────
print(defaultdict(int, Counter("aab")))
print(dict(defaultdict(int, defaultdict(int, {"a": 1, "b": 2}))))
print(dict(defaultdict(int, collections.OrderedDict([("x", 1), ("y", 2)]))))


class DuckMap:
    def keys(self):
        return ["k1", "k2"]

    def __getitem__(self, k):
        return k.upper()


print(dict(defaultdict(str, DuckMap())))

# ── factory=None acts like dict ──────────────────────────────────────────
print(defaultdict(None))
print(defaultdict(None, {"k": 9}))
print(defaultdict())

# ── factory still autocreates missing keys after init ────────────────────
d = defaultdict(int, {"a": 1})
print(d["missing"], dict(d))
d2 = defaultdict(list, [("x", [0])])
d2["new"].append(7)
print(dict(d2))

# ── bad arguments raise the right exception class / message ──────────────
def show(fn):
    try:
        fn()
    except Exception as e:
        print(type(e).__name__, e)

show(lambda: defaultdict(5))                     # non-callable factory → TypeError
show(lambda: defaultdict("nope"))                # str is not callable → TypeError
show(lambda: defaultdict(int, 1, 2))             # too many positionals → TypeError
show(lambda: defaultdict(int, 42))               # non-iterable second arg → TypeError
show(lambda: defaultdict(int, [("x", 1, 2)]))    # bad pair length → ValueError
show(lambda: defaultdict(int, [("x",)]))         # bad pair length → ValueError
