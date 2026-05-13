# collections — Counter and defaultdict (both real Python classes in
# pyrust, defined via `pyrust_module!`'s `class { … }` block).
#
# Methods that diverge from CPython on purpose are NOT exercised here
# (no Counter arithmetic operators, no name-mangled `_counts` access)
# — the parity harness asserts byte-identical output, so anything we
# print has to match CPython exactly.

from collections import Counter, defaultdict

# ── Counter: constructor forms ────────────────────────────────────────
c = Counter([1, 2, 1, 3, 2, 1])
print("count-1", c[1])
print("count-2", c[2])
print("count-3", c[3])
print("count-missing", c[99])      # 0 — the dict-subclass quirk
print("count-len", len(c))
print("contains-1", 1 in c)
print("contains-z", 99 in c)

# String input — tally characters
s = Counter('aabcccd')
print("str-a", s['a'])
print("str-c", s['c'])

# Mapping input — preserve counts as-is
m = Counter({'x': 5, 'y': 3})
print("map-x", m['x'])
print("map-y", m['y'])

# Iteration yields keys in insertion order (the original __iter__ bug
# fix — this used to silently produce nothing).
print("iter-keys", list(c))
# Re-iteration must work: each iter(c) takes a fresh snapshot.
print("iter-keys-again", list(c))

# ── Counter methods ──────────────────────────────────────────────────
print("most-common-all", c.most_common())
print("most-common-top1", c.most_common(1))
print("elements-list", sorted(c.elements()))

c2 = Counter('aab')
c2.update('bb')
print("update", sorted(c2.items()))
c2.subtract('aaaa')
print("subtract", sorted(c2.items()))

# copy independence: mutating the copy doesn't affect the original.
c3 = Counter('aab')
c4 = c3.copy()
c4['a'] = 999
print("copy-orig", c3['a'])
print("copy-new", c4['a'])

# ── Counter subscript: write-through ──────────────────────────────────
c5 = Counter()
c5['k'] = 7
print("setitem", c5['k'])

# ── defaultdict ──────────────────────────────────────────────────────
counts = defaultdict(int)
for ch in 'aabbb':
    counts[ch] += 1
print("dd-counts-a", counts['a'])
print("dd-counts-b", counts['b'])
print("dd-counts-c", counts['c'])    # default factory ran -> 0
print("dd-len", len(counts))         # 3 entries now (a, b, c)
print("dd-contains-a", 'a' in counts)
print("dd-iter", sorted(list(counts)))

# Custom factory
def make():
    return 'fresh'
d2 = defaultdict(make)
print("dd-custom", d2['anything'])

# defaultdict(None) — no factory, missing keys raise KeyError
d_none = defaultdict(None)
d_none['present'] = 1
print("dd-none-present", d_none['present'])
try:
    d_none['absent']
except KeyError as e:
    print("dd-none-keyerror", str(e))

# Non-callable factory rejected at construction.  Wording of the
# message differs (CPython: "first argument must be callable or None";
# pyrust qualifies with the module name) so we only assert the
# exception class made it out.
try:
    defaultdict(42)
except TypeError:
    print("dd-not-callable", "TypeError")

# defaultdict copy preserves factory + items
dd_copy = counts.copy()
print("dd-copy-a", dd_copy['a'])
print("dd-copy-len", len(dd_copy))
