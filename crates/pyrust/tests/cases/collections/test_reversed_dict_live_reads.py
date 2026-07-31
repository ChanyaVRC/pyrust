# Parity fixture for issue #2932.
#
# `reversed(d)` / `reversed(d.keys())` / `reversed(d.values())` /
# `reversed(d.items())` are *live* walks, not snapshots.  CPython's
# `dictreviter` is a descending index into the mapping's entry array: it reads
# the key and the value out of the entry when the cursor reaches it, so a value
# replaced mid-walk is observed.  pyrust previously materialised the whole
# forward key/value/pair list at `reversed()` time and handed back a dead
# snapshot, so a replaced value read STALE (and a deleted key was still
# yielded).
#
# What this file pins:
#
#   * the issue's repro, and the same-size overwrite matrix across all four
#     reverse walks on both `dict` and `OrderedDict` (and their subclasses);
#   * a size change still raises `RuntimeError` on the next step with each
#     container's wording, and latches the way #2915 / #2930 established --
#     plain dict re-raises forever (CPython stamps `di_used = -1`, which no
#     real length can match again), OrderedDict's mutation arm fires once and
#     then reports plain exhaustion;
#   * exhaustion, empty mappings, and reverse ordering itself.
#
# What this file deliberately does NOT pin: what a *structural* mutation that
# preserves the mapping's size (delete one key, insert another) yields from
# here on.  CPython's answer there is a property of its table geometry, not of
# any documented semantics -- deleting a key leaves a tombstone and the insert
# either appends past the cursor or triggers a compaction that renumbers every
# live entry underneath it, so the same script yields a duplicate at one size
# and not at another.  test_live_cursor_same_size_mutation.py documents the
# same regime for forward walks.  The invariant pyrust does hold, and which is
# checked below, is that no removed entry is ever yielded and no surviving
# entry is yielded twice -- the second half on a mapping built large and then
# deleted down, so that CPython has entry slots to spare, cannot compact, and
# is duplicate-free too.
#
# That freedom belongs to `dict` alone.  An OrderedDict compares its `od_state`
# node-relinking counter before anything else, so the delete retires the walk
# outright and there is no element sequence left to depend on the table -- so
# those rows ARE pinned here, exactly, since issue #2931 gave pyrust the
# counter.

from collections import OrderedDict


def step(iterator):
    try:
        return ("value", next(iterator))
    except RuntimeError as error:
        return ("RuntimeError", str(error))
    except StopIteration:
        return ("StopIteration", None)


def steps(iterator, times=3):
    return [step(iterator) for _ in range(times)]


def reverse(mapping, view):
    if view == "direct":
        return reversed(mapping)
    return reversed(getattr(mapping, view)())


VIEWS = ("direct", "keys", "values", "items")


class DictSub(dict):
    pass


class ODSub(OrderedDict):
    pass


FACTORIES = (
    ("dict", dict),
    ("OrderedDict", OrderedDict),
    ("DictSub", DictSub),
    ("ODSub", ODSub),
)


# ── The issue's repro ────────────────────────────────────────────────────────
d = {"a": 1, "b": 2}
it = reversed(d.values())
print("repro first", next(it))
d["a"] = 99
print("repro second", next(it))


# ── A value replaced mid-walk is read live, for every view and container ─────
for name, factory in FACTORIES:
    for view in VIEWS:
        mapping = factory(a=1, b=2, c=3)
        iterator = reverse(mapping, view)
        first = step(iterator)
        mapping["a"] = 99
        mapping["b"] = 88
        print("live", name, view, first, steps(iterator))

# Replacing the entry the cursor is *about* to reach, one step at a time.
for name, factory in FACTORIES:
    mapping = factory(a=1, b=2, c=3)
    iterator = reverse(mapping, "values")
    observed = [next(iterator)]
    mapping["b"] = 20
    observed.append(next(iterator))
    mapping["a"] = 10
    observed.append(next(iterator))
    print("stepwise", name, observed)

# Replacing an entry the walk has already passed is invisible; replacing one it
# has not reached is visible.
for name, factory in FACTORIES:
    mapping = factory({index: index for index in range(10)})
    iterator = reverse(mapping, "items")
    observed = [next(iterator) for _ in range(3)]
    for index in range(10):
        mapping[index] = -index
    observed.extend(iterator)
    print("bulk rewrite", name, observed)

# Values are read, not copied: a mutable value keeps its identity.
mapping = {"a": [1]}
iterator = reversed(mapping.items())
key, value = next(iterator)
mapping["a"].append(2)
print("value identity", key, value, value is mapping["a"])

# A generator suspended across the overwrite sees the new value.
def walk(mapping):
    for value in reversed(mapping.values()):
        yield value


mapping = {"a": 1, "b": 2, "c": 3}
generator = walk(mapping)
print("generator first", next(generator))
mapping["a"] = 111
print("generator rest", list(generator))

# Native consumers draining the rest of the walk read live values too.
for consumer in (list, tuple, sum):
    mapping = {"a": 1, "b": 2, "c": 3}
    iterator = reversed(mapping.values())
    next(iterator)
    mapping["a"] = 50
    print("consumer", consumer.__name__, consumer(iterator))

# Two independent reverse walks over the same mapping each read live.
mapping = {"a": 1, "b": 2, "c": 3}
values = reversed(mapping.values())
items = reversed(mapping.items())
print("aliased first", next(values), next(items))
mapping["b"] = 200
print("aliased second", next(values), next(items))
print("aliased third", next(values), next(items))

# A larger mapping (past the point where a snapshot would have been taken).
big = {index: index for index in range(200)}
iterator = reversed(big.values())
observed = [next(iterator) for _ in range(5)]
big[100] = -1
observed.extend(iterator)
print("big", observed[:5], observed.count(-1), len(observed))


# ── A size change raises, with each container's wording and latch ────────────
def mutate(mapping, kind):
    if kind == "insert":
        mapping["z"] = 9
    elif kind == "delete":
        del mapping["a"]
    elif kind == "clear":
        mapping.clear()


for name, factory in FACTORIES:
    for view in VIEWS:
        for kind in ("insert", "delete", "clear"):
            mapping = factory(a=1, b=2, c=3)
            iterator = reverse(mapping, view)
            next(iterator)
            mutate(mapping, kind)
            print("guard", name, view, kind, steps(iterator))

# The plain-dict size guard is sticky: CPython stamps the iterator, so
# restoring the original size does not un-silence it.  OrderedDict's mutation
# arm instead retires the iterator after its single report.
for name, factory in FACTORIES:
    for view in VIEWS:
        mapping = factory(a=1, b=2, c=3, d=4)
        iterator = reverse(mapping, view)
        next(iterator)
        mapping["z"] = 9
        first = steps(iterator, 1)
        del mapping["z"]
        print("latch", name, view, first, steps(iterator, 2))


# Native consumers agree with next() about the latch.
def consume(label, call):
    mapping = {"a": 1, "b": 2, "c": 3, "d": 4}
    iterator = reversed(mapping.values())
    next(iterator)
    mapping["z"] = 9
    try:
        next(iterator)
    except RuntimeError:
        pass
    try:
        print("latched consumer", label, call(iterator))
    except RuntimeError as error:
        print("latched consumer", label, "RuntimeError:", error)


for label, call in (
    ("list", list),
    ("tuple", tuple),
    ("sorted", sorted),
    ("sum", sum),
    ("any", any),
    ("comprehension", lambda source: [item for item in source]),
):
    consume(label, call)


# A for loop propagates the first raise; a value-only rewrite does not raise.
def loop(factory, kind):
    mapping = factory(a=1, b=2, c=3)
    try:
        for key in reversed(mapping):
            if key == "c":
                if kind == "value":
                    mapping["a"] = 99
                else:
                    mutate(mapping, kind)
        return "NO ERROR", dict(mapping)
    except RuntimeError as error:
        return "RuntimeError: " + str(error)


for name, factory in FACTORIES:
    for kind in ("insert", "delete", "value"):
        print("loop", name, kind, loop(factory, kind))


# ── Exhaustion and empty mappings ────────────────────────────────────────────
for name, factory in FACTORIES:
    for view in VIEWS:
        mapping = factory(a=1)
        iterator = reverse(mapping, view)
        drained = [step(iterator), step(iterator)]
        mapping["z"] = 9
        print("exhausted", name, view, drained, steps(iterator, 2))

    for view in VIEWS:
        mapping = factory()
        iterator = reverse(mapping, view)
        print("empty", name, view, list(iterator))
        mapping["x"] = 1
        print("empty after", name, view, list(iterator), step(iterator))


# ── A same-size structural mutation: only the shared invariants ──────────────
#
# The exact element sequence is CPython table geometry (see the module
# docstring) -- CPython yields a duplicate at one size and not at another --
# so what is pinned is what holds either way: the removed entry is never
# yielded afterwards, a key inserted above the cursor is never yielded, and
# every pair the walk does yield matches the live mapping.
def structural(size, cut, victim):
    mapping = {chr(97 + index): index for index in range(size)}
    iterator = reversed(mapping.items())
    observed = [next(iterator) for _ in range(cut)]
    del mapping[victim]
    mapping["z"] = 99
    observed.extend(iterator)
    keys = [key for key, _ in observed]
    return (
        victim not in keys[cut:],
        "z" not in keys,
        all(mapping.get(key) == value for key, value in observed[cut:]),
    )


for size in (3, 5, 9):
    for victim in ("a", "c", chr(97 + size - 1)):
        print("structural", size, victim, structural(size, 1, victim))


# ── The same mutation on an OrderedDict is fully determined (issue #2931) ────
#
# The freedom above is a plain dict's: its walk keeps going, so what it yields
# depends on the table. An OrderedDict compares `od_state` before it looks at
# anything else, so the delete alone retires the walk and the exact element
# sequence never comes into it. That is why these rows could not be pinned
# while pyrust detected mutation by length only.
def ordered_structural(factory, view, size, cut, victim):
    mapping = factory((chr(97 + index), index) for index in range(size))
    iterator = reverse(mapping, view)
    observed = [next(iterator) for _ in range(cut)]
    del mapping[victim]
    mapping["z"] = 99
    try:
        observed.extend(iterator)
        return len(observed), "NO ERROR"
    except RuntimeError as error:
        return len(observed), "RuntimeError: " + str(error)


for name, factory in (("OrderedDict", OrderedDict), ("ODSub", ODSub)):
    for view in VIEWS:
        for size in (3, 5, 9):
            for victim in ("a", "c", chr(97 + size - 1)):
                print(
                    "ordered structural",
                    name,
                    view,
                    size,
                    victim,
                    ordered_structural(factory, view, size, 1, victim),
                )


# A `move_to_end` mid-walk changes no length at all, so nothing but the
# entry-order counter can retire these walks.
for name, factory in (("OrderedDict", OrderedDict), ("ODSub", ODSub)):
    for view in VIEWS:
        for last in (True, False):
            mapping = factory(a=1, b=2, c=3, d=4)
            iterator = reverse(mapping, view)
            next(iterator)
            mapping.move_to_end("b", last=last)
            print("ordered move_to_end", name, view, last, steps(iterator, 3))


# ── The descending cursor re-anchors instead of holding a raw position ───────
#
# The three invariants above cannot tell a re-anchoring walk from one that
# keeps a raw index: a mapping that renumbers its entries on delete shifts the
# next live entry down into the slot the cursor already read, so re-reading
# that slot yields a *duplicate of a surviving key* -- which is not a removed
# entry, not the appended key, and does match the live mapping.  Only counting
# duplicates separates the two, and at the sizes above CPython produces some of
# its own from compaction, so the count is not comparable there.
#
# Building the mapping large and then deleting down to size fixes that: CPython
# compacts only when an insert finds no free entry slot, and a mapping that was
# built at `build` keys has slots to spare once it holds `keep`.  Its insert
# therefore appends for every table size below, and the walk is duplicate-free
# on both sides -- which is what re-anchoring on the last yielded key
# reproduces, and what a raw descending position does not.
def slack(build, keep, cut, victim):
    mapping = {"k%03d" % index: index for index in range(build)}
    for index in range(keep, build):
        del mapping["k%03d" % index]
    iterator = reversed(mapping.items())
    observed = [next(iterator) for _ in range(cut)]
    del mapping[victim]
    mapping["kZZZ"] = 99
    observed.extend(iterator)
    keys = [key for key, _ in observed]
    duplicates = sorted({key for key in keys if keys.count(key) > 1})
    return keys, duplicates


for build, keep in ((17, 5), (17, 9), (33, 9), (65, 17)):
    for cut in (1, 2, 3):
        for victim in ("k000", "k001"):
            print("slack", build, keep, cut, victim, slack(build, keep, cut, victim))


# ── The sticky size guard reaches every mapping iterator, not just dicts ─────
#
# The stamp lives on the shared guard, so `mappingproxy` -- forward and
# reversed, class-backed and dict-backed -- and an instance `__dict__` proxy
# all keep re-raising after their size moved, exactly as their dict
# counterparts do.
class Klass:
    a = 1
    b = 2


proxy = vars(Klass)
iterator = reversed(proxy)
next(iterator)
Klass.added = 9
first = steps(iterator, 1)
del Klass.added
print("proxy reversed latch", first, steps(iterator, 2))

iterator = iter(vars(Klass))
next(iterator)
Klass.added = 9
first = steps(iterator, 1)
del Klass.added
print("proxy forward latch", first, steps(iterator, 2))

mapping = {"a": 1, "b": 2, "c": 3}
iterator = reversed(mapping.keys().mapping)
next(iterator)
mapping["z"] = 9
first = steps(iterator, 1)
del mapping["z"]
print("dict proxy reversed latch", first, steps(iterator, 2))


class Plain:
    pass


instance = Plain()
instance.a = 1
instance.b = 2
instance.c = 3
iterator = iter(instance.__dict__)
next(iterator)
instance.z = 9
first = steps(iterator, 1)
del instance.z
print("instance dict latch", first, steps(iterator, 2))


# ── Reverse ordering itself ──────────────────────────────────────────────────
for name, factory in FACTORIES:
    mapping = factory(a=1, b=2, c=3)
    print(
        "order",
        name,
        [list(reverse(mapping, view)) for view in VIEWS],
    )

print("order big", list(reversed({index: index for index in range(5)}.items())))
