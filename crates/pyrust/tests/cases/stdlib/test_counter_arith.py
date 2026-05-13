# Counter arithmetic operators — issue #331.
#
# CPython gives Counter four binary operators (`+`, `-`, `&`, `|`) plus
# their in-place variants (`+=`, `-=`, `&=`, `|=`) with one quirky
# universal rule: after the per-key op is applied, entries whose count
# is `<= 0` are dropped from the result.  The parity harness asserts
# byte-identical output against CPython, so anything we print here has
# to match.
#
# Reference: https://docs.python.org/3/library/collections.html#collections.Counter

from collections import Counter


# Counter dict-order is insertion-order, but the operators don't
# promise a stable insertion order between operands (CPython walks
# LHS keys first, then RHS-only keys).  Sort for byte-identical
# parity output.
def show(label, c):
    print(label, sorted(c.items()))


# ── Binary operators ─────────────────────────────────────────────────
c = Counter('aab')      # {'a': 2, 'b': 1}
d = Counter('bcc')      # {'b': 1, 'c': 2}

show("add", c + d)            # +: sum per key, drop <= 0
show("sub", c - d)            # -: difference, drop <= 0
show("and", c & d)            # &: min, drop <= 0
show("or",  c | d)            # |: max, drop <= 0

# ── The "keep only positive counts" rule ────────────────────────────
# c - d with d's count exceeding c's must NOT yield a negative entry —
# that's the whole point of Counter arithmetic vs raw dict math.
show("sub-empty", Counter({'a': 1}) - Counter({'a': 2}))

# `+` also drops zero/negative results.  This builds on top of the
# raw "merged sum" being -1.
show("add-drop", Counter({'a': 2}) + Counter({'a': -3}))

# `|` and `&` apply the same `> 0` filter to their max/min results.
show("and-drop", Counter({'a': 1}) & Counter({'a': -1}))
show("or-drop",  Counter({'a': -2}) | Counter({'a': -1}))

# ── Disjoint keys (the "missing = 0" treatment) ─────────────────────
# Per CPython, missing counts are treated as 0 for `&` and `|` (and
# implicitly for `+`/`-`).  So `&` with disjoint keys yields an empty
# Counter, `|` yields the union of both sides.
c1 = Counter({'a': 1})
c2 = Counter({'b': 2})
show("disjoint-and", c1 & c2)
show("disjoint-or",  c1 | c2)
show("disjoint-add", c1 + c2)
show("disjoint-sub", c1 - c2)

# ── Counter `op` dict (CPython parity: TypeError) ───────────────────
# CPython actually rejects `Counter + dict`, `Counter - dict`, and
# `Counter & dict` with TypeError — Counter's `__add__` etc. only
# accept Counter operands.  (Pre-Python-3.9, `Counter | dict` would
# also raise; in 3.9+ it succeeds but returns a plain `dict` via
# `dict.__or__`, which we can't mirror without dict subclassing.)
try:
    _ = Counter({'a': 1}) + {'b': 2}
    print("expected TypeError for + dict")
except TypeError:
    print("plus-dict-typeerror")

# ── In-place operators preserve identity ────────────────────────────
# `c += d` must mutate `c` and return the same object (CPython's
# augmented-op contract).
c = Counter('aab')
c_id = id(c)
c += Counter('bcc')
show("iadd-result", c)
print("iadd-identity", id(c) == c_id)

c = Counter({'a': 5, 'b': 1})
c_id = id(c)
c -= Counter({'a': 2, 'b': 5})       # b → -4, dropped
show("isub-result", c)
print("isub-identity", id(c) == c_id)

c = Counter({'a': 2, 'b': 1})
c_id = id(c)
c &= Counter({'a': 1, 'b': 3})
show("iand-result", c)
print("iand-identity", id(c) == c_id)

c = Counter({'a': 2, 'b': 1})
c_id = id(c)
c |= Counter({'a': 1, 'b': 3})
show("ior-result", c)
print("ior-identity", id(c) == c_id)

# ── `+` returns a *new* Counter (identity changes) ──────────────────
c = Counter('a')
e = c + Counter()
print("plus-makes-new", id(c) != id(e))

# ── TypeError on non-mapping RHS ────────────────────────────────────
# Capturing the result is important: pyrust's optimizer DCEs a binary
# op whose result is discarded, so `Counter() + 1` as an expression
# statement silently elides.  Assigning forces evaluation.
try:
    _ = Counter() + 1
    print("expected TypeError for +")
except TypeError:
    print("plus-int-typeerror")

try:
    _ = Counter() - "string"
    print("expected TypeError for -")
except TypeError:
    print("sub-str-typeerror")
