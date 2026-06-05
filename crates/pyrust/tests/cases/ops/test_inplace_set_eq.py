# Issue #2244: in-place set operators (|= &= -= ^=) must dedup/compare element
# keys via user __eq__/__hash__, not by Rc-pointer identity.  The out-of-place
# ops (#1907) and storage (#1919) were already eq-aware; the in-place arm in
# `try_inplace_op` did raw IndexSet insert/contains, so sets of custom-eq
# objects mis-merged (e.g. `{K(1)} |= {K(1)}` produced len 2 instead of 1).


class K:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return type(o) is K and self.v == o.v

    def __hash__(self):
        return hash(self.v)

    def __repr__(self):
        return f"K({self.v})"


# |=  union: equal-but-distinct objects dedup.
a = {K(1)}
a |= {K(1), K(2)}
print("or:", len(a), sorted(x.v for x in a))

# &=  intersection: membership via __eq__.
b = {K(1), K(2), K(3)}
b &= {K(2), K(3), K(4)}
print("and:", len(b), sorted(x.v for x in b))

# -=  difference.
c = {K(1), K(2), K(3)}
c -= {K(2), K(3)}
print("sub:", len(c), sorted(x.v for x in c))

# ^=  symmetric difference.
d = {K(1), K(2)}
d ^= {K(2), K(3)}
print("xor:", len(d), sorted(x.v for x in d))

# Nested-object tuple keys (cf. #2059): the user object inside a tuple key must
# still force the eq-aware path.
e = {(K(1),)}
e |= {(K(1),), (K(2),)}
print("tuple:", len(e), sorted(t[0].v for t in e))

# Aliasing: |= mutates in place, so an alias observes the update.
f = {K(1)}
g = f
f |= {K(2)}
print("alias:", len(g), f is g)

# Mixed: in-place op against a frozenset operand.
h = {K(1), K(2)}
h -= frozenset({K(2)})
print("frozen-rhs:", len(h), sorted(x.v for x in h))

# Primitive-element sets are unregressed (raw fast path).
p = {1, 2, 3}
p |= {3, 4}
p &= {2, 3, 4, 5}
p -= {5}
p ^= {2, 9}
print("prim:", sorted(p))

print("done")
