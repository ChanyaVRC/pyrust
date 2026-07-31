"""Dict cursors under delete-plus-insert mid-iteration (issue #2901).

A CPython dict iterator is a raw index into the *compact entries array*.
Deleting a key leaves a tombstone in place, an insert appends past the tail,
and the iterator carries a remaining-key budget (``di_len``) that is spent one
key per yield.  Deleting a key the walk has already passed and inserting a new
one therefore keeps ``ma_used`` unchanged -- no "changed size" error -- and the
walk runs on over the surviving entries until it reaches the appended one with
its budget already spent, which is the "dictionary keys changed during
iteration" error.

pyrust's entry positions *are* compact entries, so this whole family already
reproduces cell for cell without emulating CPython's table.  A 24-size x
4-position x 3-victim sweep, a 1500-trial mutation fuzz, and the grid below
all agree.  Sections A-F pin that agreement.

Two families are **deliberately not matched**, because in CPython they are
properties of the table's geometry rather than of any documented semantics,
and in both of them CPython's answer is the worse one:

* **Compacting insert.**  When the delete pushes ``dk_nentries`` past
  ``USABLE_FRACTION(dk_size)``, the following insert rebuilds the entries
  array, sliding every later entry down under the running cursor -- so CPython
  silently *skips* a key that was present for the whole walk and finishes
  without error.  Two dicts with identical contents, identical iteration order
  and identical mutations answer differently here purely because of how they
  were built (section G), so no implementation can match this without
  replicating CPython's allocation policy.
* **Delete-then-reinsert of the same key.**  The reinsert appends a second
  entry for a key whose first entry is behind the cursor, so CPython yields
  that key *twice*.  pyrust suppresses the repeat: in the 1500-trial fuzz
  CPython duplicated an element 27 times and pyrust zero times.

For those two, only the invariants that hold either way are pinned
(sections G and H): the walk terminates, yields no duplicates, and yields
nothing that was never in the mapping.

Every key type here is insertion ordered, so nothing below depends on the
hash seed.
"""

import collections


def fill_limit(size):
    """CPython's ``dk_usable`` ceiling for a dict built by ``size`` inserts.

    An insert that would push ``dk_nentries`` past this rebuilds the entries
    array.  Deletes do not lower ``dk_nentries``, so this is a budget on total
    inserts, not on the live count.
    """
    capacity = 8
    while (2 * capacity) // 3 < size:
        capacity *= 2
    return (2 * capacity) // 3


# The fill levels at which the very next insert compacts -- CPython's
# USABLE_FRACTION(2**k).  Sections A-F stay clear of them; section G is about
# exactly them.
COMPACTING = {(2 * (1 << k)) // 3 for k in range(3, 12)}
SIZES = [n for n in (2, 3, 4, 6, 7, 8, 9, 11, 12, 16, 17, 63, 64, 65, 100)
         if n not in COMPACTING]
NEW = 10_000


def int_key(index):
    return index


def str_key(index):
    return "k%04d" % index


def tuple_key(index):
    return (index, index)


KEY_KINDS = (("int", int_key), ("str", str_key), ("tuple", tuple_key))


def classify(error):
    if error is None:
        return "none"
    text = str(error)
    if "changed size" in text:
        return "size"
    if "keys changed" in text:
        return "keys"
    return text


def drain(iterator, prefix, limit):
    """Run an iterator out, reporting the error class instead of raising."""
    observed = list(prefix)
    error = None
    try:
        for item in iterator:
            observed.append(item)
            if len(observed) > limit:
                error = "runaway"
                break
    except RuntimeError as exc:
        error = exc
    return observed, classify(error)


def cuts(size):
    """Cursor positions: the head, past the adaptive threshold, and the tail."""
    return sorted({position for position in (1, 2, 13, size - 1)
                   if 1 <= position < size})


def view_of(mapping, view):
    return mapping if view is None else getattr(mapping, view)()


def projected(key, size, view):
    """The original entries in insertion order, as the view reports them."""
    if view == "values":
        return list(range(size))
    if view == "items":
        return [(key(index), index) for index in range(size)]
    return [key(index) for index in range(size)]


# ── A. Delete a visited key, insert a new one: run on, then "keys changed" ───
#
# The surviving original entries are all yielded, in their original order; the
# appended key is never reached, because the walk's budget runs out on it.
def visited_delete(size, cut, key, view, victim_index):
    mapping = {key(index): index for index in range(size)}
    iterator = iter(view_of(mapping, view))
    prefix = [next(iterator) for _ in range(cut)]
    del mapping[key(victim_index)]
    mapping[key(NEW)] = NEW
    observed, error = drain(iterator, prefix, size + 20)
    return error, observed == projected(key, size, view)


for name, key in KEY_KINDS:
    for size in SIZES:
        for cut in cuts(size):
            for view in (None, "keys", "values", "items"):
                print("A first", name, size, cut, view,
                      visited_delete(size, cut, key, view, 0))
                print("A last ", name, size, cut, view,
                      visited_delete(size, cut, key, view, cut - 1))


# ── B. Deleting the key at or after the cursor stays a clean walk ────────────
#
# The victim's entry is still ahead of the cursor, so removing it also removes
# one owed yield; the appended key then lands exactly on the freed budget and
# *is* observed.
def ahead_delete(size, cut, victim_index):
    mapping = {index: index for index in range(size)}
    iterator = iter(mapping)
    prefix = [next(iterator) for _ in range(cut)]
    del mapping[victim_index]
    mapping[NEW] = NEW
    observed, error = drain(iterator, prefix, size + 20)
    expected = [index for index in range(size) if index != victim_index] + [NEW]
    return error, observed == expected


for size in SIZES:
    for cut in cuts(size):
        print("B cursor", size, cut, ahead_delete(size, cut, cut))
        print("B tail  ", size, cut, ahead_delete(size, cut, size - 1))


# ── C. Which mutation totals raise, and which error ──────────────────────────
#
# A net size change is reported before the walk resumes and wins over
# everything else; a net-zero change is only discovered when the cursor
# reaches the surplus entry.
def multi(size, cut, removed, added):
    mapping = {index: index for index in range(size)}
    iterator = iter(mapping)
    prefix = [next(iterator) for _ in range(cut)]
    for index in removed:
        del mapping[index]
    for offset in range(added):
        mapping[NEW + offset] = NEW + offset
    observed, error = drain(iterator, prefix, size + 20)
    return error, len(observed)


for size in SIZES:
    if size < 6:
        continue
    cut, visited, tail = 2, [0, 1], [size - 1, size - 2]
    shapes = (
        ("1v/1 ", visited[:1], 1),
        ("1v/2 ", visited[:1], 2),
        ("2v/1 ", visited, 1),
        ("2v/2 ", visited, 2),
        ("1t/1 ", tail[:1], 1),
        ("2t/2 ", tail, 2),
        ("v+t/2", [0, size - 1], 2),
        ("0/1  ", [], 1),
        ("1v/0 ", visited[:1], 0),
    )
    for label, removed, added in shapes:
        # A shape whose inserts spend the fill budget rebuilds the entries
        # array; that is section G's family, not this one.
        if size + added > fill_limit(size):
            continue
        print("C", label, size, multi(size, cut, removed, added))


# ── D. The error is one-shot, unlike the size-change latch ───────────────────
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


for size in (2, 4, 64, 65):
    mapping = {index: index for index in range(size)}
    iterator = iter(mapping)
    for _ in range(size):
        next(iterator)
    del mapping[0]
    mapping[NEW] = NEW
    print("D one-shot", size, steps(iterator))


# ── E. Every consumer of the walk agrees with the explicit one ──────────────
def consume(label, call):
    mapping = {index: index for index in range(4)}
    iterator = iter(mapping)
    next(iterator)
    del mapping[0]
    mapping[NEW] = NEW
    try:
        print("E", label, call(iterator))
    except RuntimeError as error:
        print("E", label, "RuntimeError:", error)


consume("list", list)
consume("tuple", tuple)
consume("sorted", sorted)
consume("sum", sum)
consume("max", max)
consume("comprehension", lambda source: [item for item in source])
consume("set", set)

# The statement-level loop takes a different path to the same cursor.
for size in (4, 64, 65):
    mapping = {index: index for index in range(size)}
    observed = []
    error = None
    try:
        for candidate in mapping:
            observed.append(candidate)
            if len(observed) == 1:
                del mapping[candidate]
                mapping[NEW] = NEW
    except RuntimeError as exc:
        error = exc
    print("E loop", size, classify(error), observed == list(range(size)))

# A generator suspended across the mutation resumes on the same cursor.
def walk(mapping):
    for key in mapping:
        yield key


for size in (4, 64, 65):
    mapping = {index: index for index in range(size)}
    generator = walk(mapping)
    prefix = [next(generator) for _ in range(2)]
    del mapping[prefix[0]]
    mapping[NEW] = NEW
    observed, error = drain(generator, prefix, size + 20)
    print("E generator", size, error, observed == list(range(size)))


# ── F. Subclasses and mapping-shaped collections route through the cursor ────
class DictSubclass(dict):
    pass


CARRIERS = (
    ("dict", lambda size: {index: index for index in range(size)}),
    ("subclass", lambda size: DictSubclass({index: index for index in range(size)})),
    ("Counter", lambda size: collections.Counter({index: index + 1 for index in range(size)})),
    ("defaultdict", lambda size: collections.defaultdict(
        int, {index: index for index in range(size)})),
)

for label, build in CARRIERS:
    for size in (4, 8, 64, 65):
        mapping = build(size)
        iterator = iter(mapping)
        prefix = [next(iterator)]
        del mapping[0]
        mapping[NEW] = NEW
        observed, error = drain(iterator, prefix, size + 20)
        print("F", label, size, error, observed == list(range(size)))


# ── G. Compacting insert: geometry, so only shared invariants ────────────────
#
# CPython finishes silently here having skipped one live key; pyrust reports
# the same "keys changed" error it reports one element either side of these
# fill levels.  See the module docstring.
def invariants(size, cut, victim_index):
    mapping = {index: index for index in range(size)}
    ever = set(mapping)
    iterator = iter(mapping)
    prefix = [next(iterator) for _ in range(cut)]
    del mapping[victim_index]
    mapping[NEW] = NEW
    ever.add(NEW)
    observed, error = drain(iterator, prefix, size + 20)
    return (error != "runaway",
            len(observed) == len(set(observed)),
            set(observed) <= ever,
            len(observed) == size)


for size in sorted(n for n in COMPACTING if n <= 200):
    for cut in cuts(size):
        print("G compacting", size, cut, invariants(size, cut, 0))


# ── H. Delete then reinsert the same key: geometry, so invariants only ───────
#
# CPython appends a second entry for the key and yields it twice.
def cycle_invariants(size, cut, victim_index):
    mapping = {index: index for index in range(size)}
    ever = set(mapping)
    iterator = iter(mapping)
    prefix = [next(iterator) for _ in range(cut)]
    del mapping[victim_index]
    mapping[victim_index] = victim_index
    observed, error = drain(iterator, prefix, size + 20)
    return error != "runaway", set(observed) <= ever, len(observed) >= size - 1


for size in (4, 8, 12, 64, 65):
    for cut in cuts(size):
        print("H cycle visited  ", size, cut, cycle_invariants(size, cut, 0))
        print("H cycle unvisited", size, cut, cycle_invariants(size, cut, size - 1))
