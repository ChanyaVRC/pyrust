# Binary set operators (& | - ^) follow the LEFT operand's type (issue #2042).
# `set OP frozenset` -> set; `frozenset OP set` -> frozenset.

for sym, l_set_r_fz, l_fz_r_set in [
    ("&", {1, 2} & frozenset({2, 3}), frozenset({1, 2}) & {2, 3}),
    ("|", {1, 2} | frozenset({3}), frozenset({1, 2}) | {3}),
    ("-", {1, 2} - frozenset({2}), frozenset({1, 2}) - {2}),
    ("^", {1, 2} ^ frozenset({3}), frozenset({1, 2}) ^ {3}),
]:
    print(sym, type(l_set_r_fz).__name__, type(l_fz_r_set).__name__)

# Same-type operands keep their type.
print(type({1, 2} & {2, 3}).__name__)
print(type(frozenset({1, 2}) & frozenset({2, 3})).__name__)
print(type(frozenset({1}) | frozenset({2})).__name__)

# Result contents are unchanged by the type fix.
print(sorted({1, 2, 3} & frozenset({2, 3, 4})))
print(sorted({1, 2} | frozenset({3})))
print(sorted({1, 2, 3} - frozenset({2})))
print(sorted(frozenset({1, 2}) ^ {2, 3}))

# In-place operators are unaffected: a set stays a set, a frozenset rebinds
# to a new frozenset.
s = {1, 2}
s &= frozenset({2, 3})
print(type(s).__name__, sorted(s))

fs = frozenset({1, 2})
fs |= {3}
print(type(fs).__name__, sorted(fs))

# Method forms follow the receiver's type.
print(type(frozenset({1}).union({2})).__name__)
print(type({1}.union(frozenset({2}))).__name__)
print(type(frozenset({1, 2}).intersection({1})).__name__)
print(type(frozenset({1}).symmetric_difference({2})).__name__)
print(type(frozenset({1, 2}).difference({1})).__name__)


# Object-key sets exercise the eq-aware slow path; the result type still
# follows the left operand.
class K:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        return hash(self.v)

    def __eq__(self, other):
        return isinstance(other, K) and self.v == other.v


a = {K(1), K(2)}
b = frozenset({K(2), K(3)})
print(type(a & b).__name__)
print(type(b & a).__name__)
