# Parity fixture for issue #1868: set/dict membership must honour identity
# (CPython checks `startkey IS key` before `==`), so the *same* float('nan')
# value deduplicates even though `nan == nan` is False.
#
# pyrust floats are NaN-boxed value types with no per-object identity, so the
# fix uses bit equality as the identity proxy: the same NaN value (identical
# bit pattern, e.g. a NaN bound to a name and reused) deduplicates.  Distinct
# `float('nan')` calls produce identical bits in pyrust and therefore also
# collapse — CPython keeps them separate (each call is a fresh object).  That
# distinct-object corner is intentionally NOT exercised here because it is
# unrepresentable in a value-type float model; see the PR for #1868.

# ── Same-NaN deduplication (the issue's headline repro) ─────────────────────
nan = float("nan")
print(len({nan, nan, nan}))   # 1: same value dedups via identity

s = set()
for _ in range(3):
    s.add(nan)
print(len(s))                 # 1

d = {}
for _ in range(3):
    d[nan] = 1
print(len(d))                 # 1

print(nan in {nan})           # True
print(nan in {nan: "x"})      # True

# set comprehension exercises the same insert path
print(len({nan for _ in range(3)}))   # 1

# NaN nested inside a tuple key: tuple equality propagates element-wise,
# so the same NaN deduplicates the tuple keys too.
t = (nan,)
print(len({t, t}))            # 1
print(t in {t})               # True
print(len({(nan,), (nan,)}))  # 1: same `nan` value in both tuples

# NaN as a complex component: the same complex value deduplicates too.
cnan = complex(1.0, float("nan"))
print(len({cnan, cnan, cnan}))  # 1
print(cnan in {cnan})           # True
creal_nan = complex(float("nan"), 2.0)
print(len({creal_nan, creal_nan}))  # 1
print({1 + 2j: "x"}[1 + 2j])    # x: ordinary complex key lookup

# ── Normal keys must be completely unaffected ───────────────────────────────
print(len({1, 1, 1}))         # 1
print(len({1, 1.0, True}))    # 1: cross-type numeric equality (1 == 1.0 == True)
print(len({0.0, -0.0}))       # 1: distinct bits, IEEE-equal, hash equal
print(len({"a", "a"}))        # 1
print(len({(1, 2), (1, 2)}))  # 1
print(len({frozenset({1}), frozenset({1})}))  # 1

# distinct finite floats stay distinct
print(len({1.5, 2.5, 1.5}))   # 2

# cross-type dict lookup still works
print({1.0: "one"}[1])        # one
print({True: "t"}[1])         # t

# ── User objects: dedup by __eq__ / identity, never by float bits ───────────
class C:
    def __init__(self, v):
        self.v = v
    def __eq__(self, other):
        return isinstance(other, C) and self.v == other.v
    def __hash__(self):
        return hash(self.v)

a = C(1)
print(len({a, a}))            # 1: same object
print(len({C(1), C(1)}))      # 1: distinct objects, __eq__ True
print(len({C(1), C(2)}))      # 2: __eq__ False

# custom object whose __eq__ is always False still dedups by identity
class Never:
    def __eq__(self, other):
        return False
    def __hash__(self):
        return 7

n = Never()
print(len({n, n, n}))         # 1: identity
print(n in {n})               # True
print(len({Never(), Never()}))  # 2: distinct, __eq__ False
