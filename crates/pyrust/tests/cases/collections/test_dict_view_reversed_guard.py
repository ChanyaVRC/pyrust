# Parity fixture for issue #2448.
#
# `reversed()` over a dict / OrderedDict view (`dict_keys` / `dict_items` /
# `dict_values`) — and `reversed(dict)` / `reversed(OrderedDict)` directly —
# must be guarded against size mutation during iteration exactly like forward
# iteration is.  CPython 3.12 raises `RuntimeError` on the NEXT `next()` call
# after the dict's size changes:
#   - plain dict  -> "dictionary changed size during iteration"
#   - OrderedDict -> "OrderedDict mutated during iteration"
#
# Before the fix the `reversed()` path materialised a forward `list` up front
# and handed it to an unguarded reversed-list iterator, losing the live backing
# entirely (the walk silently completed).
#
# Notes on semantics pinned here:
#   - All three views support `reversed()` in 3.12 (including `dict_values`).
#   - A value-only mutation (same key count) is NOT an error.
#   - Once the iterator has cleanly signalled StopIteration it stays exhausted:
#     a size mutation made afterwards must NOT resurrect the guard.

from collections import OrderedDict


def trial(make, view, mutate):
    """Build `reversed(view(make()))`, pull one element, mutate, pull again."""
    d = make()
    if view is None:
        it = reversed(d)
    else:
        it = reversed(view(d))
    next(it)
    mutate(d)
    try:
        next(it)
        return "no raise"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)
    except StopIteration:
        return "StopIteration"


def plain():
    return {"a": 1, "b": 2, "c": 3}


def od():
    return OrderedDict([("a", 1), ("b", 2), ("c", 3)])


VIEWS = [
    ("keys", lambda d: d.keys()),
    ("values", lambda d: d.values()),
    ("items", lambda d: d.items()),
    ("dict", None),
]

MUTATIONS = [
    ("insert", lambda d: d.__setitem__("z", 99)),
    ("delete", lambda d: d.__delitem__("a")),
]

for label, make in [("dict", plain), ("OrderedDict", od)]:
    for vname, view in VIEWS:
        for mname, mutate in MUTATIONS:
            print(label, vname, mname, "->", trial(make, view, mutate))

# Value-only mutation (key count unchanged) must NOT raise.
d = plain()
it = reversed(d.keys())
print("first", next(it))
d["a"] = 999
print("value-only", next(it))

# Exhausted iterator is safe: a clean StopIteration latches, so a later size
# mutation does not raise on the next `next()`.
d = {"only": 1}
it = reversed(d.values())
print("single", next(it))
try:
    next(it)
    print("unexpected element")
except StopIteration:
    print("exhausted")
d["new"] = 2
try:
    next(it)
    print("post-exhaust no raise")
except StopIteration:
    print("post-exhaust StopIteration")
except RuntimeError as e:
    print("post-exhaust RuntimeError:", e)

# Empty view: immediately exhausted, then mutation is safe.
d = {}
it = reversed(d.items())
print("empty list", list(it))
d["x"] = 1
print("empty after mutate", list(it))

# Correctness of the reversed ordering itself (no mutation).
d = {"a": 1, "b": 2, "c": 3}
print("rev keys", list(reversed(d.keys())))
print("rev values", list(reversed(d.values())))
print("rev items", list(reversed(d.items())))
print("rev dict", list(reversed(d)))

o = OrderedDict([("x", 10), ("y", 20)])
print("rev od keys", list(reversed(o.keys())))
print("rev od items", list(reversed(o.items())))
print("rev od", list(reversed(o)))
