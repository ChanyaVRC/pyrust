# Parity fixture for issues #920 and #921.
#
# #920: Counter.__setitem__ must accept any value (not just integers).
# #921: Counter.update(other_counter) must add actual counts, not +1/key.

from collections import Counter

# ── Issue #920: __setitem__ accepts non-integer values ────────────────
c = Counter()
c[slice(1, 2)] = 'x'
# CPython repr shows the actual value, sorted by integer-count then insertion order.
# Since 'x' is non-integer, the sort falls back to insertion order.
print(repr(c))            # Counter({slice(1, 2, None): 'x'})
print(c[slice(1, 2)])     # 'x'

# Non-integer values are stored and retrievable.
c2 = Counter({'a': 3})
c2['b'] = 'hello'
print(c2['b'])            # hello

# Integer values still work normally.
c3 = Counter()
c3['z'] = 7
print(c3['z'])            # 7

# ── Issue #921: Counter.update(counter) merges counts correctly ───────
c4 = Counter({'a': 1})
c5 = Counter({'a': 2})
c4.update(c5)
print(c4['a'])            # 3  (was 2 before fix)

# Multi-key merge.
c6 = Counter({'a': 1, 'b': 3})
c7 = Counter({'a': 2, 'c': 5})
c6.update(c7)
print(c6['a'])            # 3
print(c6['b'])            # 3
print(c6['c'])            # 5

# Counter.subtract(counter) uses actual counts (same apply_delta path).
c8 = Counter({'a': 5, 'b': 2})
c9 = Counter({'a': 2})
c8.subtract(c9)
print(c8['a'])            # 3  (was 4 before fix)
print(c8['b'])            # 2

# Subtract can go below zero.
c10 = Counter({'x': 1})
c11 = Counter({'x': 3})
c10.subtract(c11)
print(c10['x'])           # -2

# update from plain dict still works (existing behaviour).
c12 = Counter({'x': 1})
c12.update({'x': 4, 'y': 2})
print(c12['x'])           # 5
print(c12['y'])           # 2

# update from iterable still works (existing behaviour).
c13 = Counter({'a': 2})
c13.update(['a', 'b', 'b'])
print(c13['a'])           # 3
print(c13['b'])           # 2
