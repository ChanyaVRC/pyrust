# Regression test for issue #428 — list.sort() and sorted() must route
# through the same comparison helper as the `<` / `>` operators so they
# work uniformly on BigInt and nested-list elements.
#
# Before the fix, pyrust-builtins shipped its own narrower compare_values
# that handled only Int/Float/Bool/Str/Tuple — sorting BigInt or list
# elements failed at runtime even though `<` accepted them.

I48_MAX = 140_737_488_355_327

# ── BigInt sort ascending ──────────────────────────────────────────────────
big1 = I48_MAX + 1000
big2 = I48_MAX + 2000
big3 = I48_MAX + 500

lst = [big2, big3, big1]
lst.sort()
assert lst == [big3, big1, big2], repr(lst)

# ── BigInt sort descending ─────────────────────────────────────────────────
lst = [big2, big3, big1]
lst.sort(reverse=True)
assert lst == [big2, big1, big3], repr(lst)

# ── sorted() builtin with BigInts ──────────────────────────────────────────
assert sorted([big2, big1, big3]) == [big3, big1, big2]
assert sorted([big2, big1, big3], reverse=True) == [big2, big1, big3]

# ── Mixed BigInt + inline int ──────────────────────────────────────────────
mixed = [big1, 5, big2, -1, big3]
mixed.sort()
assert mixed == [-1, 5, big3, big1, big2], repr(mixed)

# ── Nested-list sort (lexicographic) ───────────────────────────────────────
nested = [[3, 1], [1, 5], [3, 2], [1, 2]]
nested.sort()
assert nested == [[1, 2], [1, 5], [3, 1], [3, 2]], repr(nested)

# Single-element nested
assert sorted([[3], [1], [2]]) == [[1], [2], [3]]

# Nested lists with differing lengths (CPython: shorter prefix wins on tie)
assert sorted([[1, 2, 3], [1, 2], [1]]) == [[1], [1, 2], [1, 2, 3]]

# ── key= path returning BigInt ─────────────────────────────────────────────
data = [{"n": big2}, {"n": big1}, {"n": big3}]
got = sorted(data, key=lambda d: d["n"])
assert got == [{"n": big3}, {"n": big1}, {"n": big2}], repr(got)

# key= returning a nested list
items = ["bb", "a", "cccc"]
got = sorted(items, key=lambda s: [len(s), s])
assert got == ["a", "bb", "cccc"], repr(got)
