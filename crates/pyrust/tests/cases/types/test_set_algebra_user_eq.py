# Parity fixture for issue #1907: set algebra dispatches user __eq__/__hash__.
#
# Binary set ops (& | - ^), their method forms (intersection/union/
# difference/symmetric_difference), and the subset/superset comparisons
# (< <= > >= / issubset / issuperset) must deduplicate and compare elements
# via user __eq__ rather than raw PyKey identity.  All-primitive sets keep
# the fast path; these fixtures exercise the user-instance slow path.


class E:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return isinstance(o, E) and self.v == o.v

    def __hash__(self):
        return hash(self.v)


a = {E(1), E(2)}
b = {E(2), E(3)}

# Binary operators.
print(sorted(x.v for x in (a & b)))  # [2]
print(sorted(x.v for x in (a | b)))  # [1, 2, 3] — no duplicate
print(sorted(x.v for x in (a - b)))  # [1]
print(sorted(x.v for x in (a ^ b)))  # [1, 3]

# Method forms (accept arbitrary iterables, here a list).
print(sorted(x.v for x in a.intersection([E(2), E(3)])))  # [2]
print(sorted(x.v for x in a.union([E(3)])))               # [1, 2, 3]
print(sorted(x.v for x in a.difference([E(2)])))          # [1]
print(sorted(x.v for x in a.symmetric_difference([E(2), E(3)])))  # [1, 3]

# Subset / superset.
print({E(1)}.issubset(a))       # True
print(a.issuperset({E(1)}))     # True
print({E(1)} < a)               # True
print({E(1), E(2)} <= a)        # True
print(a > {E(1)})               # True
print(a >= {E(1), E(2)})        # True

# Union never yields __eq__-duplicate elements.
big = {E(1)} | {E(1)} | {E(1)}
print(len(big))                 # 1

# frozenset algebra returns frozenset and dispatches __eq__.
fa = frozenset({E(1), E(2)})
fb = frozenset({E(2), E(3)})
print(type(fa & fb).__name__)                       # frozenset
print(sorted(x.v for x in (fa | fb)))               # [1, 2, 3]
print(type(fa.union([E(5)])).__name__)              # frozenset
print(fa.issubset({E(1), E(2), E(3)}))              # True

# __hash__ is consulted before __eq__: unequal hashes => never equal, and
# __eq__ is not even called.
class H:
    calls = 0

    def __init__(self, v, h):
        self.v = v
        self.h = h

    def __hash__(self):
        return self.h

    def __eq__(self, o):
        H.calls += 1
        return self.v == o.v


s1 = {H(1, 100)}
s2 = {H(2, 200)}  # different hash bucket
_ = s1 & s2
print(H.calls)                  # 0 — different hash means __eq__ skipped
print(len({H(1, 5)} | {H(1, 5)}))  # 1 — same hash, equal

# Primitive sets are unaffected.
print(sorted({1, 2, 3} & {2, 3, 4}))   # [2, 3]
print(sorted({1, 2} | {2, 3}))         # [1, 2, 3]
print(sorted({1, 2, 3} - {2}))         # [1, 3]
print(sorted({1, 2} ^ {2, 3}))         # [1, 3]
print({1, 2}.issubset({1, 2, 3}))      # True

# isdisjoint also dispatches user __eq__ (set + frozenset, set/iterable arg).
da = {E(1), E(2)}
print(da.isdisjoint({E(1)}))           # False — E(1) shared
print(da.isdisjoint({E(9)}))           # True
print(da.isdisjoint([E(2)]))           # False — iterable operand
print(frozenset({E(1), E(2)}).isdisjoint({E(1)}))  # False
print(frozenset({E(1), E(2)}).isdisjoint([E(9)]))  # True
print({1, 2}.isdisjoint({2, 3}))       # False — primitive fast path
print({1, 2}.isdisjoint(range(3, 5)))  # True
