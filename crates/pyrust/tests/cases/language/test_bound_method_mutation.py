# Regression test for issue #305: captured bound methods on mutable
# Tier 1 containers.
#
# In CPython all three of list, dict, set mutate-through a captured
# bound method.  In pyrust, only dict does (because Opaque::Dict is
# Rc<RefCell<...>> internally).  list/set use value-typed storage, so
# captured bound methods on those types operate on a private copy and
# silently discard mutations.
#
# This file pins the *currently-supported* surface so a future change
# that accidentally breaks the dict path (or the direct-call path for
# list/set) will be caught.  The list/set captured-bound-method case is
# deliberately NOT exercised here because pyrust diverges from CPython
# on it and the parity harness would (correctly) flag the divergence.
#
# See bound_method.rs module docs and issue #305 for the rationale.

# ---- dict: captured bound methods DO propagate mutations ----
d = {"a": 1}
upd = d.update
upd({"b": 2})
assert d == {"a": 1, "b": 2}

popper = d.pop
popper("a")
assert "a" not in d
assert d == {"b": 2}

# clear via captured bound method
clearer = d.clear
clearer()
assert d == {}

# ---- list: direct-call form propagates (CallMethod fast path) ----
lst = [1, 2, 3]
lst.append(4)
assert lst == [1, 2, 3, 4]
lst.extend([5, 6])
assert lst == [1, 2, 3, 4, 5, 6]
lst.pop()
assert lst == [1, 2, 3, 4, 5]

# ---- set: direct-call form propagates ----
s = {1, 2}
s.add(3)
assert s == {1, 2, 3}
s.discard(1)
assert s == {2, 3}

# ---- list/set: read-only captured bound methods are stable ----
lst2 = [3, 1, 2, 1]
counter = lst2.count
assert counter(1) == 2
indexer = lst2.index
assert indexer(2) == 2

s2 = {1, 2, 3}
disjoint = s2.isdisjoint
assert disjoint({4, 5}) is True
assert disjoint({1}) is False

print("bound-method mutation regression test OK")
