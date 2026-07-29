# The guarded int-loop versioning pass (PR #2895, issue #2887) runs a
# specialized out-of-line copy of a `for`/`while` body and side-exits into the
# original loop the moment a per-iteration type guard fails.  The two copies
# share one iterator cursor, so a side exit must resume at exactly the element
# the fast copy was about to process: never skipping one, never replaying one.
#
# Every element here is a distinct power of two and the loop sums them, so the
# binary accumulator is a positional receipt of which elements ran.  A skipped
# element clears a bit; a repeated element carries into the next.  The elements
# that fire the deopt carry their own bit too (`True` is `1 << 0`; only `False`
# contributes nothing), so the expected total does not depend on where the
# guard happens to fail.

namespace = globals()


class IntLike(int):
    """An `int` subclass: a real integer, but never the exact-int the guard admits."""


def bits(total):
    return sorted(index for index in range(80) if total >> index & 1)


# ── ForIter element guard: deopt in the middle, at the head, at the tail ──────
middle = [1 << 0, 1 << 1, IntLike(1 << 2), 1 << 3, 1 << 4, 1 << 62, 1 << 6, 1 << 7]
middle_total = 0
for middle_value in middle:
    middle_total += middle_value
print("middle", middle_total, bits(middle_total), middle_value)
print("middle globals", namespace["middle_total"] == middle_total)

head = [IntLike(1 << 0), 1 << 1, 1 << 2, 1 << 3]
head_total = 0
for head_value in head:
    head_total += head_value
print("head", bits(head_total), head_value)

tail = [1 << 0, 1 << 1, 1 << 2, IntLike(1 << 3)]
tail_total = 0
for tail_value in tail:
    tail_total += tail_value
print("tail", bits(tail_total), tail_value)

# Alternating exact / inexact elements re-enter the fast copy through the entry
# guards after every side exit.
alternating = [1 << 0, IntLike(1 << 1), 1 << 2, IntLike(1 << 3), 1 << 4, IntLike(1 << 5)]
alternating_total = 0
for alternating_value in alternating:
    alternating_total += alternating_value
print("alternating", bits(alternating_total))

# `bool` is not an exact int either, and 2**63 has already left the machine int.
promoting = [True, 1 << 1, 1 << 62, 1 << 63, False, 1 << 3]
promoting_total = 0
for promoting_value in promoting:
    promoting_total += promoting_value
print("promoting", promoting_total, bits(promoting_total), type(promoting_total).__name__)

# ── Subscript guard: the same receipt through `GetItemSeqOrExit` ─────────────
source = [1 << 0, 1 << 1, IntLike(1 << 2), 1 << 3, 1 << 62, 1 << 63, 1 << 6, 1 << 7]
subscript_total = 0
for subscript_index in range(8):
    subscript_total += source[subscript_index]
print("subscript", bits(subscript_total), subscript_index)
print("subscript globals", namespace["subscript_total"] == subscript_total)

tuple_source = (1 << 0, IntLike(1 << 1), 1 << 2, 1 << 3)
tuple_total = 0
for tuple_index in range(4):
    tuple_total += tuple_source[tuple_index]
print("tuple subscript", bits(tuple_total))

# The same shape driven by a counted `while`.  Whether its back edge fuses into
# a `CountCmpJump*` depends on the deferred syncs around it; the counted-compare
# opcodes themselves are driven from `counted_bounds()` below, where the fusion
# is not at the mercy of the rest of this module.
while_total = 0
while_index = 0
while while_index < 8:
    while_total += source[while_index]
    while_index += 1
print("while subscript", bits(while_total), while_index)


# ── The same loops inside a function, where no namespace sync is deferred ────
def function_scope():
    seen = 0
    for value in middle:
        seen += value
    indexed = 0
    for index in range(8):
        indexed += source[index]
    return bits(seen), bits(indexed)


print("function scope", function_scope())


# ── Guard edges ──────────────────────────────────────────────────────────────
# A subclass overriding `__iter__` must run its own Python iterator.
class OwnIter(list):
    def __iter__(self):
        return iter((1 << 10, 1 << 11, 1 << 12))


own_total = 0
for own_value in OwnIter([1, 2, 3]):
    own_total += own_value
print("own __iter__", bits(own_total))


# A subclass overriding `__getitem__` must run its own Python subscript.
class Shifting(list):
    def __getitem__(self, index):
        return list.__getitem__(self, index) << 8


shifting_source = Shifting([1 << 0, 1 << 1, 1 << 2])
shifting_total = 0
for shifting_index in range(3):
    shifting_total += shifting_source[shifting_index]
print("own __getitem__", bits(shifting_total))

# An iterator taken by hand and partly consumed keeps its position: the loop
# picks the cursor up where `next()` left it.
aliased = iter([1 << 0, 1 << 1, 1 << 2, 1 << 3, 1 << 4])
print("aliased first", next(aliased))
aliased_total = 0
for aliased_value in aliased:
    aliased_total += aliased_value
    if aliased_value == 1 << 2:
        break
print("aliased mid", bits(aliased_total), list(aliased))

# Two loops over one iterator continue rather than restart.
shared = iter([1 << 0, 1 << 1, 1 << 2, 1 << 3])
shared_first = 0
for shared_value in shared:
    shared_first += shared_value
    if shared_value == 1 << 1:
        break
shared_second = 0
for shared_value in shared:
    shared_second += shared_value
print("shared iterator", bits(shared_first), bits(shared_second))

# Rebinding the *name* mid-loop cannot redirect a `for`: the iterator already
# holds the original object.
rebound = [1 << 0, 1 << 1, 1 << 2, 1 << 3]
rebound_total = 0
for rebound_value in rebound:
    rebound_total += rebound_value
    if rebound_value == 1 << 1:
        rebound = [1 << 20, 1 << 21, 1 << 22, 1 << 23]
print("rebound for", bits(rebound_total), bits(sum(rebound)))

# A subscript, by contrast, reads the *live* binding every iteration, so the
# fast subscript must observe the rebinding exactly like the original loop.
indexed_source = [1 << 0, 1 << 1, 1 << 2, 1 << 3]
indexed_total = 0
for indexed_i in range(4):
    indexed_total += indexed_source[indexed_i]
    if indexed_i == 1:
        indexed_source = [1 << 20, 1 << 21, 1 << 22, 1 << 23]
print("rebound subscript", bits(indexed_total))

# Deleting the name mid-loop makes the next read raise.
deleted_source = [1 << 0, 1 << 1, 1 << 2, 1 << 3]
deleted_seen = 0
try:
    for deleted_i in range(4):
        deleted_seen += deleted_source[deleted_i]
        if deleted_i == 1:
            del deleted_source
except NameError:
    print("deleted name", bits(deleted_seen), "NameError")

# Mutating the list a `for` is walking is observed by the index cursor.
growing = [1 << 0, 1 << 1]
growing_total = 0
for growing_value in growing:
    growing_total += growing_value
    if growing_value == 1 << 1:
        growing.append(1 << 2)
        growing.append(IntLike(1 << 3))
print("growing", bits(growing_total), len(growing))

shrinking = [1 << 0, 1 << 1, 1 << 2, 1 << 3]
shrinking_total = 0
for shrinking_value in shrinking:
    shrinking_total += shrinking_value
    if shrinking_value == 1 << 1:
        del shrinking[0]
print("shrinking", bits(shrinking_total), shrinking)

# ── Zero-trip, single element, and the empty-then-populated pair ─────────────
zero_ran = 0
for zero_value in []:
    zero_ran += 1
print("zero trip", zero_ran, "zero_value" in namespace)

single_total = 0
for single_value in [1 << 5]:
    single_total += single_value
print("single", bits(single_total), single_value)

single_inexact = 0
for single_bad in [IntLike(1 << 5)]:
    single_inexact += single_bad
print("single inexact", bits(single_inexact))

for empty_index in range(0):
    print("unreachable")
print("zero-trip range", "empty_index" in namespace)

# ── i64 boundary bounds and BigInt promotion through the counted compare ─────
# Run every bound twice: once in a function, where an empty deferred-sync set
# leaves the `BinOpImm + CmpJump` back edge free to fuse into `CountCmpJump*`,
# and once at module scope, where the surrounding sync deferral may decline the
# fusion.  The two must agree — a fused counted compare is exactly the
# composition of the pair it replaces, at the i64 edges as much as anywhere.
def counted_bounds():
    seen = []
    index = (1 << 63) - 3
    while index < (1 << 63) + 2:
        seen.append(index - (1 << 63))
        index += 1
    boundary = (seen, index - (1 << 63))

    total = (1 << 63) - 4
    index = 0
    while index < 8:
        total += 1
        index += 1
    promotion = (total - (1 << 63), type(total).__name__)

    total = 0
    index = (1 << 63) + 3
    while index > (1 << 63) - 2:
        total += 1
        index -= 1
    descending = (total, index - (1 << 63))

    index = -(1 << 63)
    total = 0
    while index < -(1 << 63) + 4:
        total += 1
        index += 1
    minimum = (total, index + (1 << 63))

    # A `break`-terminated counted loop fuses into the opposite polarity, so the
    # sibling opcode runs the same bounds.
    total = (1 << 63) - 4
    index = 0
    while True:
        if index >= 8:
            break
        total += 1
        index += 1
    inverted = (total - (1 << 63), type(total).__name__, index)

    total = 0
    index = -(1 << 63)
    while True:
        if index >= -(1 << 63) + 4:
            break
        total += 1
        index += 1
    inverted_min = (total, index + (1 << 63))

    # `!=` is a counted compare too, and must stop exactly at the bound.
    total = 0
    index = (1 << 63) - 3
    while index != (1 << 63) + 2:
        total += 1
        index += 1
    unequal = (total, index - (1 << 63))
    return boundary, promotion, descending, minimum, inverted, inverted_min, unequal


print("counted bounds in a function", counted_bounds())

boundary_seen = []
boundary_index = (1 << 63) - 3
while boundary_index < (1 << 63) + 2:
    boundary_seen.append(boundary_index - (1 << 63))
    boundary_index += 1
print("i64 boundary", boundary_seen, boundary_index - (1 << 63))

promote_total = (1 << 63) - 4
promote_index = 0
while promote_index < 8:
    promote_total += 1
    promote_index += 1
print("countcmp promotion", promote_total - (1 << 63), type(promote_total).__name__)

descend_total = 0
descend_index = (1 << 63) + 3
while descend_index > (1 << 63) - 2:
    descend_total += 1
    descend_index -= 1
print("countcmp descending", descend_total, descend_index - (1 << 63))

min_index = -(1 << 63)
min_seen = 0
while min_index < -(1 << 63) + 4:
    min_seen += 1
    min_index += 1
print("i64 min bound", min_seen, min_index + (1 << 63))
