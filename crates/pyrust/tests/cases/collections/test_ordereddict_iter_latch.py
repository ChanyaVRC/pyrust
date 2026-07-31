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
# Issue #2931 added the other half of the detection: CPython's odict iterator
# compares `od_state` -- a counter bumped by every node relinking -- BEFORE it
# compares sizes, so a structural change that preserves length is reported on
# the mutation arm.  The matrices below pin both directions of that:
#
#   * a relink whose length is restored before the iterator looks again
#     (move_to_end, delete + reinsert, pop + setdefault) still reports;
#   * a mutation that only rewrites an existing key's value never bumps
#     `od_state`, so the walk stays valid -- including a `move_to_end` whose
#     key is already at the requested end, which CPython short-circuits.
#
# It also settles the case the previous revision of this header flagged as
# needing the counter: refilling a cleared mapping back to the recorded length
# un-silences an already-diagnosed iterator.  `clear()` is the one structural
# mutation that leaves `od_state` alone, but the refill's inserts do bump it,
# so CPython reports the refill on the mutation arm rather than by keeping the
# size arm sticky.  Pinned under "clear then refill" below.

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
    elif kind == "setdefault_existing":
        mapping.setdefault("a", 100)
    elif kind == "pop_missing_default":
        mapping.pop("nope", None)
    elif kind == "update_new":
        mapping.update({"z": 9})
    elif kind == "update_existing":
        mapping["a"] = 99
    elif kind == "ior_new":
        mapping |= {"z": 9}
    elif kind == "ior_existing":
        mapping |= {"a": 99}
    elif kind == "clear":
        mapping.clear()
    elif kind == "double_clear":
        # The second clear finds an empty mapping.  It changes no length, so
        # it must not displace the mark the live iterator is diagnosed against.
        mapping.clear()
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
    "setdefault_existing",
    "pop_missing_default",
    "update_new",
    "update_existing",
    "ior_new",
    "ior_existing",
    "clear",
    "double_clear",
)


# ── The full latch matrix: one raise then exhaustion, except for clear() ─────
#
# Every view/mutation pair is covered.  The reversed value and item views used
# to be excluded for the kinds that rewrite an existing value: they resolved
# their values eagerly and read stale.  Issue #2932 made them live walks, so
# they now belong in the matrix like every other view.
for name, factory in (("OrderedDict", OrderedDict), ("ODSub", ODSub)):
    for view in VIEWS:
        for kind in MUTATIONS:
            mapping = factory(a=1, b=2, c=3)
            iterator = get_iter(mapping, view)
            next(iterator)
            mutate(mapping, kind)
            print("latch", name, view, kind, steps(iterator))


# ── Length-preserving relinks report on the same arm (issue #2931) ──────────
#
# None of these changes the mapping's length by the time the iterator looks
# again, so a size comparison sees nothing.  CPython bumps `od_state` for each
# one and reports the mutation arm, which latches into exhaustion exactly like
# the size-changing relinks above.
def relink(mapping, kind):
    if kind == "move_to_end_last":
        mapping.move_to_end("a")
    elif kind == "move_to_end_first":
        mapping.move_to_end("c", last=False)
    elif kind == "move_round_trip":
        # The order is fully restored, but two nodes were relinked to do it.
        mapping.move_to_end("a")
        mapping.move_to_end("a", last=False)
    elif kind == "del_reinsert_same":
        del mapping["a"]
        mapping["a"] = 1
    elif kind == "del_reinsert_other":
        del mapping["a"]
        mapping["z"] = 9
    elif kind == "del_reinsert_behind_cursor":
        # Both edits sit strictly behind a cursor that has yielded only "a",
        # so neither the length nor the entry the cursor last read moves.
        del mapping["c"]
        mapping["z"] = 9
    elif kind == "pop_setdefault":
        mapping.pop("a")
        mapping.setdefault("a", 1)
    elif kind == "popitem_reinsert":
        mapping.popitem()
        mapping["q"] = 9
    elif kind == "popitem_first_reinsert":
        mapping.popitem(last=False)
        mapping["q"] = 9
    elif kind == "clear_refill":
        # Back to the recorded length: only the entry-order counter can tell.
        mapping.clear()
        mapping.update(a=1, b=2, c=3)


RELINKS = (
    "move_to_end_last",
    "move_to_end_first",
    "move_round_trip",
    "del_reinsert_same",
    "del_reinsert_other",
    "del_reinsert_behind_cursor",
    "pop_setdefault",
    "popitem_reinsert",
    "popitem_first_reinsert",
    "clear_refill",
)
for name, factory in (("OrderedDict", OrderedDict), ("ODSub", ODSub)):
    for view in VIEWS:
        for kind in RELINKS:
            mapping = factory(a=1, b=2, c=3)
            iterator = get_iter(mapping, view)
            next(iterator)
            relink(mapping, kind)
            print("relink", name, view, kind, steps(iterator))


# ── A mutation that only rewrites values leaves the walk valid ──────────────
#
# CPython bumps `od_state` when a node is added or removed, never when an
# existing key's value is replaced -- and `move_to_end` returns early when the
# node is already the one being moved to, so it is a no-op for `od_state` too.
# Every row here must run to completion with no RuntimeError.
def silent(mapping, kind):
    if kind == "assign_existing":
        mapping["a"] = 99
    elif kind == "setdefault_existing":
        mapping.setdefault("a", 100)
    elif kind == "update_existing":
        mapping.update({"a": 99})
    elif kind == "update_all_existing":
        mapping.update({"a": 1, "b": 2, "c": 3})
    elif kind == "update_empty":
        mapping.update({})
    elif kind == "ior_existing":
        mapping |= {"a": 99}
    elif kind == "pop_missing_default":
        mapping.pop("nope", None)
    elif kind == "get":
        mapping.get("a")
    elif kind == "read":
        mapping["a"]
    elif kind == "move_to_end_noop_last":
        mapping.move_to_end("c")
    elif kind == "move_to_end_noop_first":
        mapping.move_to_end("a", last=False)
    elif kind == "clear_empty":
        # Clearing an already-empty *other* mapping must not reach this one.
        OrderedDict().clear()


SILENT = (
    "assign_existing",
    "setdefault_existing",
    "update_existing",
    "update_all_existing",
    "update_empty",
    "ior_existing",
    "pop_missing_default",
    "get",
    "read",
    "move_to_end_noop_last",
    "move_to_end_noop_first",
    "clear_empty",
)
for name, factory in (("OrderedDict", OrderedDict), ("ODSub", ODSub)):
    for view in VIEWS:
        for kind in SILENT:
            mapping = factory(a=1, b=2, c=3)
            iterator = get_iter(mapping, view)
            next(iterator)
            silent(mapping, kind)
            print("silent", name, view, kind, steps(iterator))


# A single-element mapping has no node to move: `move_to_end` is a no-op in
# both directions, and the iterator is already exhausted anyway.
for last in (True, False):
    mapping = OrderedDict(a=1)
    iterator = iter(mapping)
    mapping.move_to_end("a", last=last)
    print("single move_to_end", last, steps(iterator, 2))

# move_to_end still raises KeyError for an absent key, before touching order.
mapping = OrderedDict(a=1, b=2, c=3)
iterator = iter(mapping)
next(iterator)
try:
    mapping.move_to_end("nope")
except KeyError as error:
    print("move_to_end missing", type(error).__name__, error)
print("move_to_end missing left the walk alone", steps(iterator, 3))


# ── The no-op short-circuit is a dict lookup, not "equal to the end key" ─────
#
# CPython reaches its `od_state`-preserving early return only when
# `_odict_find_node(od, key)` resolves to the node already at that end -- a
# dict lookup, so the hash has to agree before equality is consulted.  Two
# keys that compare equal but hash differently occupy two separate entries, so
# the move is real and the walk must retire.
class Loose:
    """Equal to everything, hashed by an explicit digest."""

    def __init__(self, digest, name):
        self.digest = digest
        self.name = name

    def __hash__(self):
        return self.digest

    def __eq__(self, other):
        return True

    def __repr__(self):
        return "Loose(" + self.name + ")"


for last in (True, False):
    first, second = Loose(1, "first"), Loose(2, "second")
    mapping = OrderedDict()
    mapping[first] = 1
    mapping[second] = 2
    iterator = iter(mapping)
    next(iterator)
    # `first` is not the last node and `second` is not the first one, so both
    # of these are real moves even though each compares equal to the end key.
    mapping.move_to_end(first if last else second, last=last)
    print("loose relink", last, [repr(key) for key in mapping], steps(iterator, 3))

# The same lookup rule keeps a key whose `__eq__` rejects foreign objects from
# ever being compared against the end key: their hashes differ, so CPython
# never asks, and neither may pyrust.
class Strict:
    """Raises rather than comparing against anything but its own type."""

    def __hash__(self):
        return 7

    def __eq__(self, other):
        if isinstance(other, Strict):
            return self is other
        raise ValueError("Strict refuses foreign comparison")

    def __repr__(self):
        return "Strict"


mapping = OrderedDict()
mapping["x"] = 1
mapping[Strict()] = 2
iterator = iter(mapping)
next(iterator)
mapping.move_to_end("x")
print("strict relink last", [repr(key) for key in mapping], steps(iterator, 3))

mapping = OrderedDict()
mapping[Strict()] = 2
mapping["x"] = 1
iterator = iter(mapping)
next(iterator)
mapping.move_to_end("x", last=False)
print("strict relink first", [repr(key) for key in mapping], steps(iterator, 3))

# A key that IS the end node still short-circuits through the hash gate, for
# the numeric-equivalence pairs a dict collapses into one entry.
for argument in (True, 1, 1.0):
    mapping = OrderedDict([("x", 0), (1, "one")])
    iterator = iter(mapping)
    next(iterator)
    mapping.move_to_end(argument)
    print("equivalent end key", repr(argument), list(mapping.items()), steps(iterator, 3))
    mapping = OrderedDict([(1, "one"), ("x", 0)])
    iterator = iter(mapping)
    next(iterator)
    mapping.move_to_end(argument, last=False)
    print("equivalent first key", repr(argument), list(mapping.items()), steps(iterator, 3))


# A `for` loop over each relink propagates the first raise out of the loop.
def relink_loop(kind):
    mapping = OrderedDict(a=1, b=2, c=3)
    try:
        for key in mapping:
            if key == "a":
                relink(mapping, kind)
        return "NO ERROR"
    except RuntimeError as error:
        return "RuntimeError: " + str(error)


for kind in RELINKS:
    print("for loop relink", kind, relink_loop(kind))


# A relink observed by a native consumer draining the iterator agrees with
# next(), on the first drain and on the retired second one.
for kind in ("move_to_end_last", "del_reinsert_same", "clear_refill"):
    mapping = OrderedDict(a=1, b=2, c=3)
    iterator = iter(mapping)
    next(iterator)
    relink(mapping, kind)
    try:
        print("relink drain", kind, list(iterator))
    except RuntimeError as error:
        print("relink drain", kind, "RuntimeError:", error)
    print("relink drain again", kind, list(iterator))


# Two iterators over the same mapping both see the one relink.
mapping = OrderedDict(a=1, b=2, c=3)
first = iter(mapping)
second = iter(mapping.items())
next(first)
next(second)
mapping.move_to_end("a")
print("relink aliased first", steps(first, 2))
print("relink aliased second", steps(second, 2))


# An iterator created AFTER the relink starts from the new order and is clean.
mapping = OrderedDict(a=1, b=2, c=3)
mapping.move_to_end("a")
iterator = iter(mapping)
print("relink before creation", steps(iterator, 4))


# An iterator already at its end never reports a later relink: exhaustion is
# tested first, for next() and for a drain alike.
for kind in ("move_to_end_last", "del_reinsert_same"):
    mapping = OrderedDict(a=1, b=2)
    iterator = iter(mapping)
    next(iterator)
    next(iterator)
    relink(mapping, kind)
    print("exhausted then relink", kind, list(iterator), step(iterator))


# A plain dict is NOT ordered-tagged, so the same length-preserving edit stays
# invisible to it — the two containers must keep disagreeing.
plain = {"a": 1, "b": 2, "c": 3}
iterator = iter(plain)
next(iterator)
del plain["c"]
plain["z"] = 9
print("plain dict same-size relink", sorted(str(entry) for entry in steps(iterator, 3)))


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


# ── A later clear() does not disturb the sticky size arm ────────────────────
#
# CPython stamps di_size = -1 on the first size report, so nothing a later
# clear() does can retire the iterator.  pyrust reconstructs the arm from a
# recorded clear mark instead, which makes "which clear is the iterator
# diagnosed against" load-bearing: a clear of an already-empty mapping changes
# no length and must not displace the earlier mark.
mapping = OrderedDict(a=1, b=2, c=3)
iterator = iter(mapping)
next(iterator)
mapping.clear()
print("clear then clear first", steps(iterator, 1))
mapping.clear()
print("clear then clear rest", steps(iterator, 2))

# Refilling between the two clears makes the insert a real structural mutation,
# so the iterator retires on the mutation arm instead of staying sticky.
mapping = OrderedDict(a=1, b=2, c=3)
iterator = iter(mapping)
next(iterator)
mapping.clear()
print("clear then refill first", steps(iterator, 1))
mapping["z"] = 9
mapping.clear()
print("clear then refill rest", steps(iterator, 2))


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
