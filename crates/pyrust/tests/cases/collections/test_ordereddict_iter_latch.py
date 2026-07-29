# Parity fixture for issue #2916.
#
# CPython's odict iterator latches its two mutated-during-iteration errors in
# OPPOSITE directions, and pyrust previously re-raised both forever:
#
#   * "OrderedDict mutated during iteration" fires ONCE.  CPython drops the
#     iterator's owning-mapping reference (Py_CLEAR(di->di_odict)), so every
#     later step reports plain exhaustion -- next() raises StopIteration and
#     native consumers see an empty iterator.
#   * "OrderedDict changed size during iteration" -- reachable only via
#     clear() -- instead stamps di_size = -1, which no real length can match
#     again, so it re-raises for the rest of the iterator's life.
#
# The second direction is the one a plain dict/set cursor uses for its own
# size guard (issue #2899), so the two containers must keep disagreeing here.
# test_ordereddict_iter_wording.py pins which message each mutation picks;
# this file pins what happens on the steps AFTER the first raise.
#
# Not covered: pyrust detects OrderedDict mutation by length, so a structural
# change that preserves length (move_to_end, del + reinsert) is not yet
# observed at all.  That is a separate false-negative from the latch direction
# and is deliberately left out of the matrix below.

from collections import OrderedDict


class ODSub(OrderedDict):
    pass


def step(iterator):
    try:
        return ("value", next(iterator))
    except RuntimeError as error:
        return ("RuntimeError", str(error))
    except StopIteration:
        return ("StopIteration", None)


def steps(iterator, times=3):
    return [step(iterator) for _ in range(times)]


def get_iter(mapping, view):
    if view == "direct":
        return iter(mapping)
    if view == "reversed":
        return reversed(mapping)
    if view.startswith("reversed_"):
        return reversed(getattr(mapping, view[len("reversed_") :])())
    return iter(getattr(mapping, view)())


def mutate(mapping, kind):
    if kind == "insert":
        mapping["z"] = 9
    elif kind == "delete":
        del mapping["c"]
    elif kind == "pop":
        mapping.pop("c")
    elif kind == "popitem":
        mapping.popitem()
    elif kind == "popitem_first":
        mapping.popitem(last=False)
    elif kind == "setdefault_new":
        mapping.setdefault("z", 9)
    elif kind == "update_new":
        mapping.update({"z": 9})
    elif kind == "update_existing":
        mapping["a"] = 99
    elif kind == "clear":
        mapping.clear()


VIEWS = (
    "direct",
    "keys",
    "values",
    "items",
    "reversed",
    "reversed_keys",
    "reversed_values",
    "reversed_items",
)
MUTATIONS = (
    "insert",
    "delete",
    "pop",
    "popitem",
    "popitem_first",
    "setdefault_new",
    "update_new",
    "update_existing",
    "clear",
)


# ── The full latch matrix: one raise then exhaustion, except for clear() ─────
def covered(view, kind):
    # reversed(view.values()/items()) resolves its values eagerly here, so a
    # value replaced mid-walk reads stale.  That is a separate defect from the
    # latch (it affects plain dict identically and predates this fixture), and
    # every other mutation kind is structural, so only this pair is skipped.
    return not (kind == "update_existing" and view in ("reversed_values", "reversed_items"))


for name, factory in (("OrderedDict", OrderedDict), ("ODSub", ODSub)):
    for view in VIEWS:
        for kind in MUTATIONS:
            if not covered(view, kind):
                continue
            mapping = factory(a=1, b=2, c=3)
            iterator = get_iter(mapping, view)
            next(iterator)
            mutate(mapping, kind)
            print("latch", name, view, kind, steps(iterator))


# ── The poison survives restoring the original size ─────────────────────────
#
# A size-based guard would fall silent again once the length matches; CPython
# has already retired the iterator by then.
mapping = OrderedDict(a=1, b=2, c=3)
iterator = iter(mapping)
next(iterator)
mapping["z"] = 9
steps(iterator, 1)
del mapping["z"]
print("poison survives restore", steps(iterator, 2))


# ── Native consumers agree with next() on both arms ──────────────────────────
def consume(label, call, kind):
    mapping = OrderedDict(a=1, b=2, c=3)
    iterator = iter(mapping)
    next(iterator)
    mutate(mapping, kind)
    try:
        next(iterator)
    except RuntimeError:
        pass
    try:
        print("consumer", kind, label, call(iterator))
    except RuntimeError as error:
        print("consumer", kind, label, "RuntimeError:", error)


CONSUMERS = (
    ("list", list),
    ("tuple", tuple),
    ("sorted", sorted),
    ("any", any),
    ("comprehension", lambda source: [item for item in source]),
)
for kind in ("insert", "clear"):
    for label, call in CONSUMERS:
        consume(label, call, kind)


# A consumer draining an iterator that never raised through next() latches the
# same way: the first drain reports, the second sees an empty iterator.
mapping = OrderedDict(a=1, b=2, c=3)
iterator = iter(mapping)
next(iterator)
mapping["z"] = 9
try:
    print("first drain", list(iterator))
except RuntimeError as error:
    print("first drain RuntimeError:", error)
print("second drain", list(iterator))


# ── Exhaustion is tested before the guard, for next() and for consumers ──────
#
# An iterator already sitting at its end never reports a later mutation, so a
# drain of one yields nothing rather than raising.
for kind in ("insert", "clear"):
    mapping = OrderedDict(a=1, b=2)
    iterator = iter(mapping)
    next(iterator)
    next(iterator)
    mutate(mapping, kind)
    print("exhausted then", kind, list(iterator), step(iterator))


# ── Independent iterators latch independently ────────────────────────────────
mapping = OrderedDict(a=1, b=2, c=3)
first = iter(mapping)
second = iter(mapping)
next(first)
next(second)
mapping["z"] = 9
print("aliased first", steps(first, 2))
print("aliased second", steps(second, 2))


# ── A for loop propagates the first raise out of the loop ────────────────────
def loop(kind):
    mapping = OrderedDict(a=1, b=2, c=3)
    try:
        for key in mapping:
            if key == "a":
                mutate(mapping, kind)
        return "NO ERROR"
    except RuntimeError as error:
        return "RuntimeError: " + str(error)


for kind in ("insert", "delete", "clear"):
    print("for loop", kind, loop(kind))


# A generator suspended across the mutation raises once and then closes.
def walk(mapping):
    for key in mapping:
        yield key


for kind in ("insert", "clear"):
    mapping = OrderedDict(a=1, b=2, c=3)
    generator = walk(mapping)
    next(generator)
    mutate(mapping, kind)
    print("generator", kind, steps(generator))


# ── Plain dict and set keep the opposite (sticky) latch from #2899 ───────────
for name, build, grow in (
    ("dict", lambda: {"a": 1, "b": 2, "c": 3}, lambda c: c.__setitem__("z", 9)),
    ("set", lambda: {"a", "b", "c"}, lambda c: c.add("z")),
):
    container = build()
    iterator = iter(container)
    next(iterator)
    grow(container)
    print("unordered latch", name, steps(iterator))

for view in ("keys", "values", "items"):
    mapping = {"a": 1, "b": 2, "c": 3}
    iterator = iter(getattr(mapping, view)())
    next(iterator)
    del mapping["c"]
    print("unordered view latch", view, steps(iterator))
