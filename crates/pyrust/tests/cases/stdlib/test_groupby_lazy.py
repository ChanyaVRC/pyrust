# itertools.groupby — lazy `_grouper` sub-iterator semantics (#1877).
#
# CPython yields each group as a lazy `_grouper` that shares the single
# underlying cursor with the parent groupby.  Because the cursor is shared,
# advancing the parent makes any previously yielded group stale (it stops
# yielding).  These cases pin that behaviour; they avoid asserting on the
# grouper's repr (hex address / module qualifier are platform/impl details).
from itertools import groupby

# ── the common idiom: fully consume each group in order ────────────────
print("basic", [(k, list(g)) for k, g in groupby([1, 1, 2, 3, 3])])
print("str", [(k, list(g)) for k, g in groupby("aaabbbcccd")])

# ── key function ───────────────────────────────────────────────────────
print("key", [(k, list(g)) for k, g in groupby([1, 2, 3, 4, 5], key=lambda n: n % 2)])

# ── degenerate inputs ──────────────────────────────────────────────────
print("empty", list(groupby([])))
print("single", [(k, list(g)) for k, g in groupby([9])])
print("all-same", [(k, list(g)) for k, g in groupby([5, 5, 5])])
print("all-distinct", [(k, list(g)) for k, g in groupby([1, 2, 3])])

# ── the group IS an iterator, not a list ───────────────────────────────
gb = groupby([1, 1, 2])
_, g = next(gb)
print("is-iter", iter(g) is g, hasattr(g, "__next__"))

# ── staleness: materialise all (key, group) pairs first, then consume ──
# CPython: groupers are stale once the parent advanced past them.
groups = list(groupby("aabbc"))
print("stale-after-list", [list(g) for k, g in groups])

# ── staleness: advance the parent before consuming the held group ──────
it = groupby("aabbc")
k1, g1 = next(it)
k2, g2 = next(it)
print("stale-advance", k1, list(g1))

# ── partial consumption, then advance ──────────────────────────────────
it = groupby([1, 1, 1, 2, 2])
k1, g1 = next(it)
print("partial-first", k1, next(g1))
k2, g2 = next(it)
print("partial-second", k2, list(g2))
print("partial-stale", list(g1))

# ── non-hashable keys are fine (groupby uses ==, not hashing) ──────────
print("nonhashable", [(k, list(g)) for k, g in groupby([[1], [1], [2]])])

# ── key function raising propagates ────────────────────────────────────
def bad(x):
    if x == 2:
        raise ValueError("boom")
    return x


try:
    list(groupby([1, 1, 2], key=bad))
except ValueError as e:
    print("key-raises", e)

# ── nested in a comprehension (join the chars of each run) ─────────────
print("join", [(k, "".join(g)) for k, g in groupby("aaabbbcccd")])


# ── lazy pull timing: the source iterator is advanced only on demand, ──
# not eagerly read one element ahead.  Observable via a side-effecting
# generator: after yielding one value from a group, exactly that many
# elements have been pulled from the source (CPython advances lazily).
def logged(seq, log):
    for x in seq:
        log.append(x)
        yield x


log = []
gb = groupby(logged([10, 10, 11], log), key=lambda _: 0)
_, g = next(gb)
print("pull-after-outer", list(log))
print("pull-val", next(g))
print("pull-after-1", list(log))
print("pull-val", next(g))
print("pull-after-2", list(log))
