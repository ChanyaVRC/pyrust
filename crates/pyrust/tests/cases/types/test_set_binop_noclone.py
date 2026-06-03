# Parity fixture for issue #1978: set binary operators (& | - ^) no longer
# clone both whole operands per op — they borrow the backing IndexSets and
# clone only result elements.  This exercises the borrow-based primitive fast
# path plus the same-object case (two shared borrows of one RefCell) and the
# fall-through to the eq-aware clone path for object-key sets, which must stay
# correct even when a user __eq__ mutates an operand mid-operation.

a = set(range(10))
b = set(range(5, 15))

# Primitive fast path — both directions.
print(sorted(a & b))
print(sorted(a | b))
print(sorted(a - b))
print(sorted(a ^ b))
print(sorted(b - a))
print(sorted(b ^ a))

# Same object on both sides: two shared borrows of one backing RefCell.
print(sorted(a & a))
print(sorted(a | a))
print(sorted(a - a))
print(sorted(a ^ a))

# Empty operands.
e = set()
print(sorted(a & e), sorted(a | e), sorted(a - e), sorted(a ^ e), sorted(e - a), sorted(e ^ a))

# frozenset promotion: any frozenset operand promotes the result to frozenset
# (current pyrust semantics — result type follows the frozenset operand).
fa = frozenset(range(10))
fb = frozenset(range(5, 15))
print(sorted(fa & fb), type(fa & fb).__name__)
print(sorted(fa | fb), type(fa | fb).__name__)
print(sorted(fa - fb), type(fa - fb).__name__)
print(sorted(fa ^ fb), type(fa ^ fb).__name__)

# set subclass operands (PyInstance backing) still take the borrow path.
class MySet(set):
    pass

ms = MySet(range(10))
print(sorted(ms & b))
print(sorted(ms | b))
print(sorted(ms - b))
print(sorted(ms ^ b))

# Object-key sets fall through to the eq-aware clone path.
class K:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        return self.v % 3

    def __eq__(self, o):
        return isinstance(o, K) and self.v == o.v


s1 = {K(1), K(2), K(3), K(4)}
s2 = {K(3), K(4), K(5), K(6)}
print(sorted(x.v for x in (s1 & s2)))
print(sorted(x.v for x in (s1 | s2)))
print(sorted(x.v for x in (s1 - s2)))
print(sorted(x.v for x in (s1 ^ s2)))

# Mixed primitive + object keys still dispatch __eq__ for the object key.
m1 = {K(1), 100}
m2 = {K(1), 200}
print(sorted((x.v if isinstance(x, K) else x) for x in (m1 & m2)))
print(sorted((x.v if isinstance(x, K) else x) for x in (m1 - m2)))

# Re-entrancy: a user __eq__ that mutates an operand mid-op must not corrupt
# the result (object-key sets use the clone path, so the live set is free to
# grow).  CPython tolerates this; pyrust must match.
class Evil:
    def __init__(self, v, target=None):
        self.v = v
        self.target = target

    def __hash__(self):
        return 0

    def __eq__(self, o):
        if self.target is not None:
            self.target.add(Evil(999))
        return isinstance(o, Evil) and self.v == o.v


tb = {Evil(1), Evil(2)}
ta = {Evil(1, target=tb), Evil(3)}
print(sorted(e.v for e in (ta & tb)))
print(sorted(e.v for e in (ta - tb)))

# TypeError for a non-set RHS on a real set operator.
try:
    a & [1, 2]
except TypeError as ex:
    print("TypeError:", ex)
