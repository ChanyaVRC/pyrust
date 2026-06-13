# Parity fixture for issue #2447.
#
# `Counter` and `defaultdict` are plain `dict` subclasses in CPython, so their
# `keys()` / `values()` / `items()` views are PLAIN `dict_keys` / `dict_values`
# / `dict_items` (not a special name), and mutating the container during view
# iteration raises the plain "dictionary changed size during iteration"
# RuntimeError.
#
# Before the fix:
#   - Counter views were eager `list` snapshots (wrong type, never live, never
#     guarded).
#   - defaultdict views were live but `store_items` replaced the backing `Rc`
#     on every mutation, so the live view detached and the guard never fired
#     (the stale-`Rc` sub-case #2447 split out of #2436).  They were also
#     mistagged `ordered=true`, which would have emitted the OrderedDict
#     wording once guarded.
#
# The matrix below exercises both shapes x three views x {insert, delete} for
# the `for`-loop (GetIter) and the manual `iter()`/`next()` forms, plus view
# type names, set-ops, liveness through update/subtract/clear, the getattr-bound
# route, the inline-cache double call, and defaultdict's missing-key insertion
# during iteration.

from collections import Counter, defaultdict


def make_counter():
    return Counter(a=1, b=2, c=3)


def make_ddict():
    return defaultdict(int, {"a": 1, "b": 2, "c": 3})


SHAPES = [("Counter", make_counter), ("defaultdict", make_ddict)]


def get_view(d, view):
    if view == "keys":
        return d.keys()
    if view == "values":
        return d.values()
    return d.items()


def mutate(d, mut):
    if mut == "insert":
        d["z"] = 9
    else:
        del d["a"]


def trial_for(make, view, mut):
    d = make()
    try:
        for _ in get_view(d, view):
            mutate(d, mut)
        return "NO ERROR"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)
    except Exception as e:
        return type(e).__name__ + ": " + str(e)


def trial_next(make, view, mut):
    d = make()
    it = iter(get_view(d, view))
    try:
        next(it)
        mutate(d, mut)
        next(it)
        return "NO ERROR"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)
    except Exception as e:
        return type(e).__name__ + ": " + str(e)


for label, trial in (("for", trial_for), ("next", trial_next)):
    for name, make in SHAPES:
        for view in ("keys", "values", "items"):
            for mut in ("insert", "delete"):
                print(label, name, view, mut, "->", trial(make, view, mut))


# View TYPE names: plain dict views, not list, not a special odict name.
for name, make in SHAPES:
    d = make()
    print(name, "keys type:", type(d.keys()).__name__)
    print(name, "values type:", type(d.values()).__name__)
    print(name, "items type:", type(d.items()).__name__)


# keys() and items() views are set-like; values() is not.
c1 = Counter(a=1, b=2, c=3)
c2 = Counter(b=1, c=1, d=1)
print("keys &:", sorted(c1.keys() & c2.keys()))
print("keys |:", sorted(c1.keys() | c2.keys()))
print("keys -:", sorted(c1.keys() - c2.keys()))
print("keys ^:", sorted(c1.keys() ^ c2.keys()))
print("items &:", sorted(Counter(a=1, b=2).items() & Counter(a=1, b=9).items()))
try:
    Counter(a=1).values() & Counter(a=1).values()
    print("values & : worked (wrong)")
except TypeError:
    print("values &: TypeError")


# Liveness through update / subtract / clear (CPython keeps views attached).
c = Counter(a=1, b=2)
ks = c.keys()
c.update({"x": 5})
print("Counter keys after update:", sorted(ks))
c.subtract({"a": 1})
print("Counter keys after subtract:", sorted(ks))
c.clear()
print("Counter keys after clear:", list(ks))

d = defaultdict(int, {"a": 1, "b": 2})
ds = d.keys()
d.update({"x": 5})
print("ddict keys after update:", sorted(ds))
d.clear()
print("ddict keys after clear:", list(ds))


# getattr-bound route: bind the method, then call it — still a live view.
c = Counter(a=1, b=2)
bound = c.keys
print("getattr-bound type:", type(bound()).__name__)
kv = bound()
c["z"] = 5
print("getattr-bound liveness:", sorted(list(kv)))


# Inline-cache fast path: the same call site twice must both build live views.
def cached(coll):
    return sorted(coll.items())


cc = Counter(a=1, b=2)
print("cached call 1:", cached(cc))
print("cached call 2:", cached(cc))


# defaultdict missing-key read during iteration INSERTS via default_factory and
# so changes size mid-loop — CPython raises (Counter[missing] returns 0 and does
# NOT insert, so a Counter read is safe).
d = defaultdict(int, {"a": 1, "b": 2})
try:
    it = iter(d.keys())
    next(it)
    _ = d["missing"]  # factory inserts -> size change
    next(it)
    print("ddict missing-during-iter: NO ERROR")
except RuntimeError as e:
    print("ddict missing-during-iter:", e)

c = Counter(a=1, b=2)
try:
    for k in c.keys():
        _ = c["missing"]  # returns 0, does NOT insert
    print("Counter missing-during-iter: NO ERROR")
except RuntimeError as e:
    print("Counter missing-during-iter:", e)


# repr of the views.
print(repr(Counter(a=1, b=2).keys()))
print(repr(Counter(a=1, b=2).values()))
print(repr(Counter(a=1, b=2).items()))
