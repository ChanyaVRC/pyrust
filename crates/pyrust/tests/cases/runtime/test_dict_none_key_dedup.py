# Parity fixture: dict/set None-key cross-variant dedup (issues #906, #934).
#
# PR #929 made d[None] = v enter a cross-variant dedup scan to handle the rare
# case where a PyKey::Object with hash == hash(None) was inserted first.
# Issue #934: the scan was unconditional, causing a ~47% regression on tight
# d[None] = v loops.  The fix (PR #934) short-circuits when no such cross-variant
# Object entry exists in the dict.
#
# This fixture verifies correctness in both the common case and the rare
# cross-variant collision case.

# --- Common case: None key is deduplicated by IndexMap natively ---
d = {}
d[None] = 1
d[None] = 2
print(len(d))     # 1
print(d[None])    # 2

# --- None key in a dict with other keys; still exactly one None entry ---
d2 = {"a": 10, "b": 20}
d2[None] = 99
d2[None] = 100
print(len(d2))        # 3
print(d2[None])       # 100
print(d2["a"])        # 10

# --- Cross-variant collision: Object with hash == hash(None) that __eq__-matches None.
# When inserted before None, the later d[None] = v must overwrite the Object entry
# (not add a second entry), preserving insertion order.
none_hash = hash(None)

class NoneEq:
    """An object that hashes to hash(None) and compares equal to None."""
    def __hash__(self):
        return none_hash
    def __eq__(self, other):
        return other is None or isinstance(other, NoneEq)

obj = NoneEq()
d3 = {}
d3[obj] = "obj"
# At this point: one entry, keyed by obj (a PyKey::Object)
print(len(d3))       # 1
# Now insert None — must collapse with the existing obj entry (same hash, __eq__ True)
d3[None] = "none"
print(len(d3))       # 1  (must NOT become 2)
print(d3[None])      # "none"

# --- Reverse: None first, then Object with same hash and __eq__ True ---
d4 = {}
d4[None] = "first"
d4[obj] = "second"
print(len(d4))       # 1  (must NOT become 2)

# --- set: None dedup in a plain set ---
s = {None, None, None}
print(len(s))        # 1
s.add(None)
print(len(s))        # 1

# --- set: cross-variant collision ---
s2 = set()
s2.add(obj)
print(len(s2))       # 1
s2.add(None)
print(len(s2))       # 1  (must collapse)
