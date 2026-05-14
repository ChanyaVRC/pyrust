# `a.append(a)` and friends preserve the self-alias (#414).
#
# Before the fix, `unalias_args_for_mutation` deep-copied any arg
# sharing the receiver's Rc-backing, regardless of whether the called
# method actually iterated the arg.  This broke self-aliased element
# additions (`append` / `insert` / `add`) by replacing the alias with
# an unrelated fresh copy.
#
# The fix only unaliases for methods that genuinely iterate their args
# through the receiver's storage (`extend` / `update` / the set
# `*_update` variants).  Other mutating methods pass their arg through
# untouched.

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

# 4. list.extend(self) is the original use case for the unalias dance:
# must observe the pre-extend snapshot, not loop forever.
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

# ─── Cyclic structure via append (#364 cycle detection + #414 alias) ─────

# `repr` on a self-cycle now actually exercises the cycle-detection
# code path that PR #412 added — before the alias fix, the deep copy
# meant `a.append(a)` never produced a real cycle to detect.
cycle = []
cycle.append(cycle)
assert repr(cycle) == "[[...]]"

print("ok")
