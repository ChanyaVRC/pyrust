# Regression test for issue #305: captured bound methods on mutable
# Tier 1 containers (list, dict, set).
#
# Before the fix in this branch, `Value::clone` on a list or set
# deep-copied the backing storage, so a captured bound method like
# `m = lst.append` held a private copy of `lst` and its mutations
# silently disappeared.  CPython has no such bug because list/dict/set
# are PyObject* references (clones share storage).
#
# The structural fix (Option 1 of #305) routes list and set storage
# through `Rc<…RefCell<…>>`, the same shape dict already used.  After
# the fix, `Value::clone` on these types shares the backing — so
# captured bound methods mutate the original.
#
# This file pins the fixed behaviour against CPython.  Every assert
# below produces output identical to CPython's, which is what the
# parity harness checks.

# ---- list: captured bound methods now propagate mutations ----
lst = [1, 2, 3]
m = lst.append
m(4)
assert lst == [1, 2, 3, 4]

ext = lst.extend
ext([5, 6])
assert lst == [1, 2, 3, 4, 5, 6]

ins = lst.insert
ins(0, 0)
assert lst == [0, 1, 2, 3, 4, 5, 6]

rem = lst.remove
rem(3)
assert lst == [0, 1, 2, 4, 5, 6]

pop = lst.pop
pop()
assert lst == [0, 1, 2, 4, 5]

rev = lst.reverse
rev()
assert lst == [5, 4, 2, 1, 0]

srt = lst.sort
srt()
assert lst == [0, 1, 2, 4, 5]

clr = lst.clear
clr()
assert lst == []

# ---- dict: captured bound methods (already worked, pinned here too) ----
d = {"a": 1}
upd = d.update
upd({"b": 2})
assert d == {"a": 1, "b": 2}

popper = d.pop
popper("a")
assert "a" not in d
assert d == {"b": 2}

clearer = d.clear
clearer()
assert d == {}

# ---- set: captured bound methods now propagate mutations ----
s = {1, 2}
adder = s.add
adder(3)
assert s == {1, 2, 3}

disc = s.discard
disc(1)
assert s == {2, 3}

upd_s = s.update
upd_s({4, 5})
assert s == {2, 3, 4, 5}

clr_s = s.clear
clr_s()
assert s == set()

# ---- aliasing: b = a; mutation through b is visible via a ----
a = [1, 2, 3]
b = a
b.append(4)
assert a == [1, 2, 3, 4]
assert a is b
assert id(a) == id(b)

da = {"x": 1}
db = da
db["y"] = 2
assert da == {"x": 1, "y": 2}
assert id(da) == id(db)

sa = {1, 2}
sb = sa
sb.add(3)
assert sa == {1, 2, 3}
assert id(sa) == id(sb)

# ---- direct-call form still propagates (CallMethod fast path) ----
lst2 = [1, 2, 3]
lst2.append(4)
assert lst2 == [1, 2, 3, 4]

s2 = {1, 2}
s2.add(3)
assert s2 == {1, 2, 3}

# ---- read-only captured bound methods still work ----
lst3 = [3, 1, 2, 1]
counter = lst3.count
assert counter(1) == 2
indexer = lst3.index
assert indexer(2) == 2

s3 = {1, 2, 3}
disjoint = s3.isdisjoint
assert disjoint({4, 5}) is True
assert disjoint({1}) is False

# ---- list: function-parameter mutations propagate to the caller ----
# Both forms — direct `l.append(...)` and the captured-bound-method form
# `a = l.append; a(...)` — flow through the Rc-shared backing, so the
# caller's `lst_f` / `lst_f2` sees the mutation.
def f(l: list):
    l.append(42)


lst_f = [1, 2, 3]
f(lst_f)
assert lst_f == [1, 2, 3, 42]


def f2(l: list):
    a = l.append
    a(99)


lst_f2 = [1, 2, 3]
f2(lst_f2)
assert lst_f2 == [1, 2, 3, 99]

print("bound-method mutation regression test OK")
