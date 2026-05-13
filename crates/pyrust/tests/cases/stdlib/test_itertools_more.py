# itertools — the rest of the API (#330).
#
# Each function is a class with `__iter__`/`__next__` dunders that drive
# the source iterator one element at a time, matching CPython's lazy
# semantics where the algorithm allows.  Combinatorics (`product`,
# `combinations`, `permutations`, `combinations_with_replacement`) walk
# an eagerly-materialised pool by index — equivalent in semantics to
# CPython, just realised differently.

from itertools import (
    accumulate, combinations, combinations_with_replacement, count,
    cycle, dropwhile, groupby, islice, permutations, product,
    repeat, starmap, takewhile,
)


# ── count ─────────────────────────────────────────────────────────────
print("count-int", list(islice(count(), 5)))
print("count-start", list(islice(count(10), 4)))
print("count-step", list(islice(count(0, 2), 5)))
print("count-neg-step", list(islice(count(10, -3), 4)))
print("count-float", list(islice(count(0.0, 0.5), 4)))


# ── repeat ────────────────────────────────────────────────────────────
print("repeat-N", list(repeat("x", 4)))
print("repeat-0", list(repeat("x", 0)))
print("repeat-neg", list(repeat("x", -1)))     # CPython: empty
# Infinite repeat exercised via islice
print("repeat-inf", list(islice(repeat(7), 3)))


# ── cycle ─────────────────────────────────────────────────────────────
print("cycle-str", list(islice(cycle("abc"), 7)))
print("cycle-list", list(islice(cycle([1, 2]), 5)))
print("cycle-empty", list(islice(cycle([]), 5)))    # never yields


# ── takewhile / dropwhile ─────────────────────────────────────────────
print("takewhile", list(takewhile(lambda x: x < 5, [1, 4, 6, 4, 1])))
print("takewhile-all", list(takewhile(lambda x: True, [1, 2, 3])))
print("takewhile-none", list(takewhile(lambda x: False, [1, 2, 3])))

print("dropwhile", list(dropwhile(lambda x: x < 5, [1, 4, 6, 4, 1])))
print("dropwhile-all", list(dropwhile(lambda x: True, [1, 2, 3])))
print("dropwhile-none", list(dropwhile(lambda x: False, [1, 2, 3])))


# ── starmap ───────────────────────────────────────────────────────────
print("starmap-mul", list(starmap(lambda a, b: a * b, [(1, 2), (3, 4)])))
print("starmap-triple", list(starmap(lambda a, b, c: a + b + c, [(1, 2, 3), (10, 20, 30)])))


# ── accumulate ────────────────────────────────────────────────────────
print("accumulate-add", list(accumulate([1, 2, 3, 4])))
print("accumulate-mul", list(accumulate([1, 2, 3, 4], lambda a, b: a * b)))
print("accumulate-init", list(accumulate([1, 2, 3], initial=100)))
print("accumulate-single", list(accumulate([42])))
print("accumulate-empty", list(accumulate([])))
print("accumulate-empty-init", list(accumulate([], initial=99)))


# ── product ───────────────────────────────────────────────────────────
print("product-pair", list(product([1, 2], [3, 4])))
print("product-triple", list(product("AB", "12")))
print("product-repeat", list(product([0, 1], repeat=3)))
print("product-empty", list(product()))            # one empty tuple
print("product-with-empty", list(product([], [1])))   # empty
print("product-repeat-0", list(product([1, 2], repeat=0)))   # one empty tuple


# ── combinations ──────────────────────────────────────────────────────
print("comb-2", list(combinations("ABCD", 2)))
print("comb-3", list(combinations([1, 2, 3, 4], 3)))
print("comb-r-eq-n", list(combinations([1, 2, 3], 3)))
print("comb-r-gt-n", list(combinations([1, 2], 3)))    # empty
print("comb-r-0", list(combinations([1, 2, 3], 0)))    # one empty tuple
print("comb-empty", list(combinations([], 0)))         # one empty tuple


# ── combinations_with_replacement ─────────────────────────────────────
print("cwr-2", list(combinations_with_replacement("ABC", 2)))
print("cwr-3", list(combinations_with_replacement([1, 2], 3)))
print("cwr-r-0", list(combinations_with_replacement([1, 2, 3], 0)))    # one empty
print("cwr-empty", list(combinations_with_replacement([], 2)))         # empty


# ── permutations ──────────────────────────────────────────────────────
print("perm-2", list(permutations("ABC", 2)))
print("perm-full", list(permutations([1, 2, 3])))
print("perm-r-eq-n", list(permutations([1, 2, 3], 3)))
print("perm-r-gt-n", list(permutations([1, 2], 3)))    # empty
print("perm-r-0", list(permutations([1, 2, 3], 0)))    # one empty tuple


# ── groupby ───────────────────────────────────────────────────────────
print("groupby-str", [(k, list(g)) for k, g in groupby("AAAABBBCCDAABBB")])
print("groupby-empty", [(k, list(g)) for k, g in groupby([])])
print(
    "groupby-key",
    [(k, list(g)) for k, g in groupby([1, 2, 3, 4, 5, 6], key=lambda n: n % 2)],
)
print("groupby-single", [(k, list(g)) for k, g in groupby([1])])
