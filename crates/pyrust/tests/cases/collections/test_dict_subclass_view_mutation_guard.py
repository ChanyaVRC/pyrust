# Parity fixture for issue #2436.
#
# Mutation-during-iteration guards must fire for the keys()/values()/items()
# *views* of OrderedDict and of dict/OrderedDict *subclasses*, not only for
# plain dict.  Before PR #2450 those view paths materialised an unguarded list
# snapshot, so iteration silently completed where CPython 3.12 raises a
# RuntimeError.  The container-specific wording must match CPython:
#   - dict / dict-subclass        -> "dictionary changed size during iteration"
#   - OrderedDict / OD-subclass   -> "OrderedDict mutated during iteration"
#
# This fixture is the "probe matrix from the #2400 review": all four container
# shapes x three views x several size-changing mutations (insert / del / pop /
# update), exercised through both the for-loop (GetIter) and manual
# iter()/next() forms, plus the no-op (value-only) mutation that must NOT raise.

from collections import OrderedDict


class DSub(dict):
    pass


class ODSub(OrderedDict):
    # custom __init__ to confirm the backing store is still installed/shared
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)


SHAPES = [
    ("dict", dict),
    ("DSub", DSub),
    ("OrderedDict", OrderedDict),
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
    elif mut == "del":
        del d["a"]
    elif mut == "pop":
        d.pop("b")
    elif mut == "update":
        d.update({"y": 7})
    else:  # value-only (no size change) -> must not raise
        d["a"] = 99


def trial_for(factory, view, mut):
    d = make(factory)
    try:
        for _ in get_view(d, view):
            mutate(d, mut)
        return "NO ERROR"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)
    except Exception as e:  # surface any wrong exception class
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
            for mut in ("insert", "del", "pop", "update", "value-only"):
                print(label, name, view, mut, "->", trial(factory, view, mut))
