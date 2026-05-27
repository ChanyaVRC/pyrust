# Augmented assignment on mutable built-in containers must mutate in-place
# and preserve object identity across aliases (#1372).
#
# Python guarantees: after `lst += x`, `lst is lst_alias` is still True.
# pyrust previously created a new object, leaving aliases stale.

# ─── list += iterable ───────────────────────────────────────────────────────

lst = [1, 2]
alias = lst
lst += [3, 4]
assert lst is alias, "list += list: identity"
assert lst == [1, 2, 3, 4], "list += list: value"

lst = [1, 2]
alias = lst
lst += (3, 4)
assert lst is alias, "list += tuple: identity"
assert lst == [1, 2, 3, 4], "list += tuple: value"

lst = [1, 2]
alias = lst
lst += {3}
assert lst is alias, "list += set: identity"

lst = [1, 2]
alias = lst
lst += range(3, 5)
assert lst is alias, "list += range: identity"
assert lst == [1, 2, 3, 4], "list += range: value"

# All aliases see the same update
lst = [1, 2]
a = lst
b = lst
lst += [3]
assert a is lst and b is lst, "triple alias after +="
assert a == [1, 2, 3] and b == [1, 2, 3], "triple alias value"

# ─── list *= n ─────────────────────────────────────────────────────────────

lst = [1, 2]
alias = lst
lst *= 3
assert lst is alias, "list *= 3: identity"
assert lst == [1, 2, 1, 2, 1, 2], "list *= 3: value"

lst = [1, 2]
alias = lst
lst *= 0
assert lst is alias, "list *= 0: identity"
assert lst == [], "list *= 0: value"

lst = [1, 2]
alias = lst
lst *= -1
assert lst is alias, "list *= -1: identity"
assert lst == [], "list *= -1: value"

lst = [1, 2]
alias = lst
lst *= 1
assert lst is alias, "list *= 1: identity"
assert lst == [1, 2], "list *= 1: value"

# ─── set |= set ─────────────────────────────────────────────────────────────

s = {1, 2}
alias = s
s |= {3, 4}
assert s is alias, "set |= set: identity"
assert s == {1, 2, 3, 4}, "set |= set: value"

s = {1, 2}
alias = s
s |= frozenset({3})
assert s is alias, "set |= frozenset: identity"
assert s == {1, 2, 3}, "set |= frozenset: value"

# ─── set &= set ─────────────────────────────────────────────────────────────

s = {1, 2, 3}
alias = s
s &= {2, 3, 4}
assert s is alias, "set &= set: identity"
assert s == {2, 3}, "set &= set: value"

# ─── set -= set ─────────────────────────────────────────────────────────────

s = {1, 2, 3}
alias = s
s -= {2}
assert s is alias, "set -= set: identity"
assert s == {1, 3}, "set -= set: value"

# ─── set ^= set ─────────────────────────────────────────────────────────────

s = {1, 2, 3}
alias = s
s ^= {2, 4}
assert s is alias, "set ^= set: identity"
assert s == {1, 3, 4}, "set ^= set: value"

# ─── TypeError when RHS is not a set/frozenset ─────────────────────────────

try:
    s = {1, 2}
    s |= [3]
    assert False, "set |= list should raise TypeError"
except TypeError:
    pass

try:
    s = {1, 2}
    s &= [1]
    assert False, "set &= list should raise TypeError"
except TypeError:
    pass

try:
    s = {1, 2}
    s -= [1]
    assert False, "set -= list should raise TypeError"
except TypeError:
    pass

try:
    s = {1, 2}
    s ^= [1]
    assert False, "set ^= list should raise TypeError"
except TypeError:
    pass

print("ok")
