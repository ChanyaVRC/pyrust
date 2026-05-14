# Self-aliased mutation (#448 — supersedes the closed #443 point fix).
#
# Each call site for mutating list/dict/set methods now goes through a
# scoped `RefCell::borrow_mut()` rather than holding an unguarded
# `&mut Vec/Map/Set` across crate boundaries.  The previous
# `unalias_args_for_mutation` pre-emptive deep copy is no longer
# needed: methods that iterate their args (`extend`, `update`,
# `*_update`) snapshot the iterable BEFORE opening the receiver's
# borrow, so self-aliased calls work without breaking the alias.

# ─── Self-alias preserving ───────────────────────────────────────────────

# 1. list.append(self): the appended slot IS the same object.
a = []
a.append(a)
assert a is a[0]
assert len(a) == 1

# 2. list.insert(0, self) preserves the alias too.
b = [0]
b.insert(0, b)
assert b is b[0]
assert len(b) == 2

# 3. dict subscript-assign with self as value preserves alias.
d = {}
d["self"] = d
assert d is d["self"]

# ─── Iterating methods still terminate safely ───────────────────────────

# 4. list.extend(self) sees the pre-extend snapshot, not the
# post-extend Vec — same as CPython.
c = [1, 2]
c.extend(c)
assert c == [1, 2, 1, 2]

# 5. dict.update(self) is a no-op (already has every key).
e = {"x": 1, "y": 2}
e.update(e)
assert e == {"x": 1, "y": 2}

# 6. set.update(self) is a no-op.
s = {1, 2, 3}
s.update(s)
assert sorted(s) == [1, 2, 3]

# 7. set.intersection_update(self) is a no-op (self ∩ self == self).
t = {1, 2, 3}
t.intersection_update(t)
assert sorted(t) == [1, 2, 3]

# 8. set.difference_update(self) empties the set (self - self == ∅).
u = {1, 2, 3}
u.difference_update(u)
assert u == set()

# ─── Cyclic structure via append (#412 cycle detection + #414/#448) ──────

# `repr` on a self-cycle now exercises the cycle-detection code path
# that PR #412 added — without #448 the deep copy meant
# `a.append(a)` never produced a real cycle to detect.
cycle = []
cycle.append(cycle)
assert repr(cycle) == "[[...]]"

print("ok")
