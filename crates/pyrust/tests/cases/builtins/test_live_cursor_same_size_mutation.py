"""Live dict/set cursors under same-size mutation (issue #2899).

A dict or set iterator here is a *live key cursor*: it follows the container's
current key order instead of a snapshot taken at ``iter()``.  This file pins
the part of that contract CPython 3.12 defines, and documents the part it
does not.

Defined by CPython, and pinned below:

* a size change observed by the next step raises ``RuntimeError`` with distinct
  dict and set wording, and *latches*: every later step re-raises it, including
  after the container is restored to its original size and for every native
  consumer that drains the iterator;
* the "dictionary keys changed during iteration" error, by contrast, is
  one-shot -- it fires once and the iterator then reports plain exhaustion;
* a mutation that nets out to no size change is invisible;
* replacing a key the walk has not reached yet makes the replacement visible;
* retiring a dict's original key quota and reinserting raises
  "dictionary keys changed during iteration";
* none of the above depends on the cursor's internal representation, so every
  case runs at sizes straddling the eager key-order threshold (64) and over
  int, str and tuple keys.

*Not* defined by CPython: whether replacing an element the walk has **already
visited** is observed.  A CPython set is open addressed and its iterator is a
raw slot cursor, so the replacement is observed exactly when it does not land
in the slot the discard just freed -- a function of the added value's probe
chain, and of whether the insert tripped a table resize.  A CPython dict can
likewise compact on reinsert once the deleted entry pushes ``dk_nentries``
past its usable fraction, which relocates every later entry under the running
cursor.  Both outcomes are properties of CPython's table geometry, not of any
documented semantics -- CPython yields a duplicate element in that regime.
Containers here are insertion ordered, so a replacement appended after the
cursor is always observed.  Only invariants that hold either way are pinned
for that family.

Set cases use int keys throughout: CPython set iteration order is hash order,
which is seed-dependent for str.  Dict cases are insertion ordered and so are
seed-stable for every key type.
"""

SIZES = (2, 4, 63, 64, 65, 100)


def guard(label, run):
    try:
        run()
    except RuntimeError as error:
        print(label, "RuntimeError:", error)
    else:
        print(label, "no error")


# ── Size-change guards keep their distinct wording at every size ─────────────
for size in SIZES:

    def set_grow(size=size):
        values = set(range(size))
        for _ in values:
            values.add(10_000)

    def set_shrink(size=size):
        values = set(range(size))
        for value in values:
            values.discard(value)

    def dict_grow(size=size):
        mapping = {index: index for index in range(size)}
        for _ in mapping:
            mapping[10_000] = 0

    def dict_shrink(size=size):
        mapping = {index: index for index in range(size)}
        for key in mapping:
            del mapping[key]

    guard("set grow %d" % size, set_grow)
    guard("set shrink %d" % size, set_shrink)
    guard("dict grow %d" % size, dict_grow)
    guard("dict shrink %d" % size, dict_shrink)


# ── A size change latches; the keys-changed error is one-shot ────────────────
#
# CPython stamps a size change into the iterator (``di_used``/``si_used`` = -1)
# and re-raises for the rest of its life.  The "keys changed" error instead
# drops the iterator's container reference, so it fires once and the iterator
# reports plain exhaustion afterwards.
def steps(iterator, times=3):
    seen = []
    for _ in range(times):
        try:
            seen.append(("value", next(iterator)))
        except RuntimeError as error:
            seen.append(("RuntimeError", str(error)))
        except StopIteration:
            seen.append(("StopIteration", None))
    return seen


for size in SIZES:
    mapping = {index: index for index in range(size)}
    iterator = iter(mapping)
    next(iterator)
    mapping[10_000] = 0
    print("dict latch", size, steps(iterator))

    values = set(range(size))
    iterator = iter(values)
    next(iterator)
    values.add(10_000)
    print("set latch", size, steps(iterator))

for view in ("keys", "values", "items"):
    mapping = {index: index for index in range(4)}
    iterator = iter(getattr(mapping, view)())
    next(iterator)
    del mapping[3]
    print("view latch", view, steps(iterator))

# The latch survives restoring the original size.
mapping = {index: index for index in range(4)}
iterator = iter(mapping)
next(iterator)
mapping[10_000] = 0
steps(iterator, 1)
del mapping[10_000]
print("dict latch restored", steps(iterator, 2))

values = set(range(4))
iterator = iter(values)
next(iterator)
values.add(10_000)
steps(iterator, 1)
values.discard(10_000)
print("set latch restored", steps(iterator, 2))


# Native consumers of a latched iterator re-raise rather than see it empty.
def consume(label, call):
    mapping = {index: index for index in range(4)}
    iterator = iter(mapping)
    next(iterator)
    mapping[10_000] = 0
    try:
        next(iterator)
    except RuntimeError:
        pass
    try:
        print("consumer", label, call(iterator))
    except RuntimeError as error:
        print("consumer", label, "RuntimeError:", error)


consume("list", list)
consume("tuple", tuple)
consume("set", set)
consume("sorted", sorted)
consume("sum", sum)
consume("any", any)
consume("comprehension", lambda source: [item for item in source])
consume("loop", lambda source: [item for item in source] and None)

# By contrast the keys-changed error is one-shot.
mapping = {1: 1, 2: 2}
iterator = iter(mapping)
next(iterator)
del mapping[1]
mapping[3] = 3
print("keys-changed one-shot", steps(iterator))

# Subclasses route through the same cursor.
class DictSubclass(dict):
    pass


class SetSubclass(set):
    pass


mapping = DictSubclass({index: index for index in range(4)})
iterator = iter(mapping)
next(iterator)
mapping[10_000] = 0
print("dict subclass latch", steps(iterator))

values = SetSubclass(range(4))
iterator = iter(values)
next(iterator)
values.add(10_000)
print("set subclass latch", steps(iterator))


# ── A mutation that nets out to no size change is invisible ──────────────────
for size in SIZES:
    values = set(range(size))
    observed = []
    for value in values:
        observed.append(value)
        values.add(10_000)
        values.discard(10_000)
    print("set net-zero", size, sorted(observed) == sorted(range(size)))

    mapping = {index: index for index in range(size)}
    keys = []
    for key in mapping:
        keys.append(key)
        mapping[10_000] = 0
        del mapping[10_000]
    print("dict net-zero", size, keys == list(range(size)))


# ── Replacing a key the walk has not reached yet is observed ─────────────────
def replace_unvisited_set(size, cut):
    values = set(range(size))
    iterator = iter(values)
    prefix = [next(iterator) for _ in range(cut)]
    victim = next(value for value in values if value not in prefix)
    values.discard(victim)
    values.add(10_000)
    observed = prefix + list(iterator)
    return (
        len(observed),
        sorted(observed) == sorted(values),
        10_000 in observed,
        victim in observed,
    )


def cuts(size):
    """Before the walk adapts its representation, after it, and at the end."""
    return sorted({1, min(13, size - 1), size - 1})


for size in SIZES:
    for cut in cuts(size):
        print("set unvisited", size, cut, replace_unvisited_set(size, cut))


def int_key(index):
    return index


def str_key(index):
    return "k%04d" % index


def tuple_key(index):
    return (index, index)


KEY_KINDS = (("int", int_key), ("str", str_key), ("tuple", tuple_key))


def replace_unvisited_dict(size, cut, key, view):
    mapping = {key(index): index for index in range(size)}
    source = mapping if view is None else getattr(mapping, view)()
    iterator = iter(source)
    prefix = [next(iterator) for _ in range(cut)]
    # The last original key is always unvisited while cut < size.
    victim = key(size - 1)
    del mapping[victim]
    mapping[key(10_000)] = 10_000
    suffix = list(iterator)
    return len(prefix) + len(suffix), len(suffix)


for name, key in KEY_KINDS:
    for size in SIZES:
        for cut in sorted({1, min(13, size - 1)}):
            for view in (None, "keys", "values", "items"):
                print(
                    "dict unvisited",
                    name,
                    size,
                    cut,
                    view,
                    replace_unvisited_dict(size, cut, key, view),
                )


# ── Retiring a dict's key quota and reinserting is a keys-changed error ──────
for name, key in KEY_KINDS:
    for size in (2, 64, 65):
        mapping = {key(index): index for index in range(size)}
        iterator = iter(mapping)
        drained = [next(iterator) for _ in range(size)]
        del mapping[key(0)]
        mapping[key(10_000)] = 0
        try:
            next(iterator)
        except RuntimeError as error:
            print("dict quota", name, size, len(drained), str(error))
        else:
            print("dict quota", name, size, len(drained), "no error")


# ── Replacing an already visited element: only shared invariants ─────────────
#
# Whether the replacement is observed is CPython table geometry (see module
# docstring), so neither the element count nor duplicate-freeness is pinned.
# What holds either way: the walk finishes without error, and every element it
# yields was in the container at some point.
def replace_visited(size, cut):
    values = set(range(size))
    ever = set(values)
    iterator = iter(values)
    prefix = [next(iterator) for _ in range(cut)]
    values.discard(prefix[-1])
    values.add(10_000)
    ever.add(10_000)
    error = None
    observed = list(prefix)
    try:
        observed.extend(iterator)
    except RuntimeError as exc:
        error = str(exc)
    return error, set(observed) <= ever, len(observed) >= size - 1


for size in SIZES:
    for cut in cuts(size):
        print("set visited", size, cut, replace_visited(size, cut))


# ── Independent cursors and a generator suspended across the mutation ────────
for size in (4, 65, 100):
    mapping = {index: index for index in range(size)}
    first = iter(mapping)
    second = iter(mapping)
    ahead = [next(first) for _ in range(min(20, size - 1))]
    behind = [next(second) for _ in range(2)]
    del mapping[size - 1]
    mapping[10_000] = 10_000
    ahead.extend(first)
    behind.extend(second)
    print("aliased", size, len(ahead), len(behind), ahead[-1], behind[-1])


def walk(mapping):
    for key in mapping:
        yield key


for size in (4, 65, 100):
    mapping = {index: index for index in range(size)}
    generator = walk(mapping)
    observed = [next(generator) for _ in range(2)]
    del mapping[size - 1]
    mapping[10_000] = 10_000
    observed.extend(generator)
    print("generator", size, len(observed), observed[-1], len(set(observed)))
