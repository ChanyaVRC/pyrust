# Parity fixture for the `|` (union) fast path: instead of rebuilding the result
# from scratch (re-hashing every LHS element), `a | b` now clones the LHS
# backing table wholesale (preserving its already-computed hashes) and adds only
# the RHS elements.  This exercises union over a variety of operand shapes and
# pins the LHS-first insertion order the clone path must preserve.

# Disjoint, overlapping, and identical operands.
print(sorted({1, 2, 3} | {4, 5, 6}))
print(sorted({1, 2, 3} | {2, 3, 4}))
print(sorted({1, 2, 3} | {1, 2, 3}))

# Asymmetric sizes: large LHS / small RHS and vice versa.
big = set(range(200))
small = {198, 199, 200, 201}
print(sorted(big | small))
print(sorted(small | big))
print(len(big | small), len(small | big))

# Empty operand on either side (LHS clone of an empty set, and RHS empty).
print(sorted(set() | {7, 8}))
print(sorted({7, 8} | set()))
print(sorted(set() | set()))

# Same object on both sides: union must dedup to the original.
s = {10, 20, 30}
print(sorted(s | s))

# Chained unions (LHS of each `|` is a freshly built set).
print(sorted({1} | {2} | {3} | {4}))

# frozenset operand promotes the result to frozenset; plain `|` stays a set.
print(type({1, 2} | {3}).__name__)
print(type(frozenset({1, 2}) | {3}).__name__, sorted(frozenset({1, 2}) | {3}))

# set subclass on the LHS (PyInstance backing) still unions correctly.
class MySet(set):
    pass


print(sorted(MySet({1, 2, 3}) | {3, 4}))

# Mixed primitive key types (int / float / str / tuple) in a single union.
print(sorted({1, "a", (1, 2)} | {2, "b", (1, 2)}, key=str))

# Object-__eq__ keys fall through to the eq-aware union path (NOT the clone
# fast path); distinct instances that compare equal collapse to one entry.
class K:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        return self.v % 4

    def __eq__(self, o):
        return isinstance(o, K) and self.v == o.v


print(sorted(x.v for x in ({K(1), K(2)} | {K(2), K(3)})))

# Augmented union (|=) over the same fast path.
acc = {1, 2}
acc |= {2, 3, 4}
print(sorted(acc))
