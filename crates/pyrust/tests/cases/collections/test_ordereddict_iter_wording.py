# Parity fixture for issue #2465.
#
# CPython 3.12's OrderedDict iterators raise *two* different RuntimeError
# messages depending on HOW the dict was mutated mid-iteration:
#   - OrderedDict.clear()              -> "OrderedDict changed size during iteration"
#   - insert / del / pop / popitem     -> "OrderedDict mutated during iteration"
# (a plain dict always says "dictionary changed size during iteration").
#
# Before the fix pyrust always said "mutated", so the clear() case diverged.
# The matrix below exercises clear vs. every other size-changing mutation across
# direct iteration, the three views, manual iter()/next(), and reversed(), for
# OrderedDict, an OrderedDict subclass, and plain dict (which must be unaffected).

from collections import OrderedDict


class ODSub(OrderedDict):
    pass


def make(factory):
    return factory(a=1, b=2, c=3)


def get_iter(d, view):
    if view == "direct":
        return iter(d)
    if view == "reversed":
        return reversed(d)
    return iter(getattr(d, view)())


def do_mutation(d, mut):
    if mut == "clear":
        d.clear()
    elif mut == "insert":
        d["z"] = 9
    elif mut == "delete":
        del d["a"]
    elif mut == "pop":
        d.pop("a")
    elif mut == "popitem":
        d.popitem()
    elif mut == "update_existing":
        d["a"] = 99


def trial_for(factory, view, mut):
    d = make(factory)
    try:
        if view == "direct":
            for _ in d:
                do_mutation(d, mut)
        elif view == "reversed":
            for _ in reversed(d):
                do_mutation(d, mut)
        else:
            for _ in getattr(d, view)():
                do_mutation(d, mut)
        return "NO ERROR"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)
    except Exception as e:
        return type(e).__name__ + ": " + str(e)


def trial_next(factory, view, mut):
    d = make(factory)
    it = get_iter(d, view)
    try:
        next(it)
        do_mutation(d, mut)
        next(it)
        return "NO ERROR"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)
    except Exception as e:
        return type(e).__name__ + ": " + str(e)


SHAPES = [
    ("dict", dict),
    ("OrderedDict", OrderedDict),
    ("ODSub", ODSub),
]
VIEWS = ("direct", "keys", "values", "items", "reversed")
MUTATIONS = ("clear", "insert", "delete", "pop", "popitem", "update_existing")

for kind, trial in (("for", trial_for), ("next", trial_next)):
    for name, factory in SHAPES:
        for view in VIEWS:
            for mut in MUTATIONS:
                print(kind, name, view, mut, "->", trial(factory, view, mut))


# Deleting every key one at a time (size reaches 0 WITHOUT clear()) must still
# say "mutated", not "changed size" — the discriminator is the operation, not
# the resulting empty size.
def del_all():
    od = OrderedDict(a=1, b=2, c=3)
    try:
        for _ in od.keys():
            for k in list(od):
                del od[k]
        return "NO ERROR"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)


print("del-all-one-by-one:", del_all())


# A clear() OUTSIDE iteration, then a fresh loop with a different mutation, must
# not let the earlier clear bleed into the new iteration's wording.
def relooped():
    od = OrderedDict(a=1, b=2, c=3)
    for _ in od:
        break
    od.clear()
    od = OrderedDict(x=1, y=2, z=3)
    try:
        for _ in od:
            del od["x"]
        return "NO ERROR"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)


print("relooped-del-after-clear:", relooped())


# Single-element clear: the sole element is yielded and the iterator is already
# exhausted before the guard is consulted, so this completes silently (CPython
# odict iterators test exhaustion first) — for OrderedDict only.
def single_clear(factory):
    d = factory(a=1)
    try:
        for _ in d:
            d.clear()
        return "NO ERROR"
    except RuntimeError as e:
        return "RuntimeError: " + str(e)


for name, factory in SHAPES:
    print("single-elem clear", name, "->", single_clear(factory))
