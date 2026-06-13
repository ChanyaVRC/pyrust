# Parity fixture for issue #2436.
#
# Mutating a dict during iteration over one of its *views* (`keys()` /
# `values()` / `items()`) must raise the same size-mutation RuntimeError as
# CPython 3.12, with the container-specific wording:
#   - dict and any plain dict subclass     -> "dictionary changed size during iteration"
#   - OrderedDict and any OrderedDict subclass -> "OrderedDict mutated during iteration"
#
# Before the fix, views of OrderedDict and of dict/OrderedDict *subclasses*
# materialised a plain `list` snapshot instead of a live, Rc-shared view, so
# iteration silently completed where CPython raises.  The matrix below exercises
# all four container shapes x three views x {insert, delete} for both the
# `for`-loop (GetIter) form and the manual `iter()` / `next()` form, plus the
# inline-cache fast path (the same function called twice).

from collections import OrderedDict


class DSub(dict):
    pass


class ODSub(OrderedDict):
    pass


SHAPES = [
    ("dict", dict),
    ("OrderedDict", OrderedDict),
    ("DSub", DSub),
    ("ODSub", ODSub),
]


def make(factory):
    return factory(a=1, b=2, c=3)


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


def trial_for(factory, view, mut):
    d = make(factory)
    try:
        for _ in get_view(d, view):
            mutate(d, mut)
        return "NO ERROR"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)
    except Exception as e:
        return type(e).__name__ + ": " + str(e)


def trial_next(factory, view, mut):
    d = make(factory)
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
    for name, factory in SHAPES:
        for view in ("keys", "values", "items"):
            for mut in ("insert", "delete"):
                print(label, name, view, mut, "->", trial(factory, view, mut))


# Inline-cache fast path: the second call to the same function must build a
# live view too (the cached `keys`/`values`/`items` dispatch previously drifted
# to the list-snapshot path, leaving the guard absent on the second call).
def cached_trial():
    d = ODSub(a=1, b=2, c=3)
    try:
        for _ in d.keys():
            del d["a"]
        return "NO ERROR"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)


print("cached call 1:", cached_trial())
print("cached call 2:", cached_trial())


# Views stay live and usable as ordinary (non-mutating) iterables.
od = OrderedDict(a=1, b=2, c=3)
print("keys:", list(od.keys()))
print("values:", list(od.values()))
print("items:", list(od.items()))
print("membership:", "a" in od.keys())
od["d"] = 4
print("live after insert:", len(od.keys()), list(od.keys()))
