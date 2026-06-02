# Issue #2060: dict/set with user-__hash__/__eq__ keys must hash-probe (only
# call __eq__ on entries in the same hash bucket), not linear-scan every entry.
# This fixture verifies the probe stays correct across collisions, dedup,
# insertion order, set membership, the constant-hash worst case, and __eq__
# error propagation — i.e. all the #2039/#368/#906 semantics are preserved.


class K:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        # Deliberate collisions: many distinct v share a bucket (v % 8).
        return self.v % 8

    def __eq__(self, other):
        return isinstance(other, K) and self.v == other.v


# Build a dict of many colliding object keys, then look them all up.
d = {}
for i in range(40):
    d[K(i)] = i * 10
print(len(d))                       # 40 — __eq__ separates same-hash keys
print(d[K(0)], d[K(7)], d[K(39)])   # 0 70 390
print(K(40) in d, K(-1) in d)       # False False

# Re-insert an existing key: dedup must overwrite, not append.
d[K(17)] = 999
print(len(d), d[K(17)])             # 40 999

# Insertion order is preserved (IndexMap) — first 5 keys in order.
print([k.v for k in list(d)[:5]])   # [0, 1, 2, 3, 4]

# Delete by a distinct-but-equal key.
del d[K(3)]
print(len(d), K(3) in d)            # 39 False

# Set of colliding object members.
s = set()
for i in range(40):
    s.add(K(i))
    s.add(K(i))                     # duplicate add is a no-op
print(len(s))                       # 40
print(K(20) in s, K(99) in s)      # True False
s.discard(K(20))
print(len(s), K(20) in s)          # 39 False

# Worst case: every key collides into one bucket; __eq__ still separates.
class One:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        return 0

    def __eq__(self, other):
        return isinstance(other, One) and self.v == other.v


one = {}
for i in range(30):
    one[One(i)] = i
print(len(one), one[One(15)])       # 30 15

# __eq__ raising propagates from a lookup that probes the bucket.
class Boom:
    def __hash__(self):
        return 5

    def __eq__(self, other):
        raise ValueError("eq boom")


bad = {Boom(): 1}
try:
    Boom() in bad
    print("no-raise")
except ValueError as exc:
    print("raised", exc)            # raised eq boom
