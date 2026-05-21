"""Parity fixture: slice keys in Counter and defaultdict (issue #905).

After PR #891 removed `SliceOps::to_key` from the pure hash path,
collections methods that called `Value::to_key()` directly rejected
slice keys with `TypeError: unhashable type`.  This fixture covers the
four affected call sites.
"""
import collections

# --- Counter.__init__ iterable path ---
c = collections.Counter([slice(1, 2)])
print("Counter([slice(1,2)]):", repr(c))

# Multiple slice elements — counts aggregate correctly.
c2 = collections.Counter([slice(1, 2), slice(1, 2), slice(3, 4)])
print("Counter repeated slices:", repr(c2))

# --- Counter.update iterable path (apply_delta) ---
c3 = collections.Counter()
c3.update([slice(3, 4)])
print("c3.update([slice(3,4)]):", repr(c3))

c3.update([slice(3, 4)])
print("c3.update again:", repr(c3))

# --- defaultdict.__missing__ with slice key ---
dd = collections.defaultdict(list)
result = dd[slice(1, 2)]
print("defaultdict(list)[slice(1,2)]:", result)
# Value is stored; second access returns the same list (identity).
print("dd[slice(1,2)] is same:", dd[slice(1, 2)] is result)

# --- Counter.__getitem__ missing-key → 0 via require_key ---
c4 = collections.Counter([1, 2, 2])
print("c4[slice(1,2)]:", c4[slice(1, 2)])

# Present slice key resolves correctly too.
c5 = collections.Counter([slice(0, 1), slice(0, 1)])
print("c5[slice(0,1)]:", c5[slice(0, 1)])

# --- defaultdict.__setitem__ and __contains__ with slice key ---
dd2 = collections.defaultdict(int)
dd2[slice(1, 3)] = 99
print("dd2[slice(1,3)]:", dd2[slice(1, 3)])
print("slice(1,3) in dd2:", slice(1, 3) in dd2)
print("slice(2,3) in dd2:", slice(2, 3) in dd2)

# --- Unhashable slice bound surfaces correct error (list start) ---
try:
    _ = collections.Counter([slice([1, 2], 3)])
    print("FAIL: expected TypeError")
except TypeError as e:
    print("unhashable bound error:", e)

# --- Counter.subtract iterable path (apply_delta with sign=-1) ---
c6 = collections.Counter([slice(1, 2), slice(1, 2)])
c6.subtract([slice(1, 2)])
print("c6 after subtract:", repr(c6))
