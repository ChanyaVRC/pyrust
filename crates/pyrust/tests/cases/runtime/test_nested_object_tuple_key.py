# A user object nested inside a tuple key dispatches __eq__/__hash__, not
# identity, for dict/set lookup, membership, dedup and set algebra. Issue #2059.


class K:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        return hash(self.v)

    def __eq__(self, o):
        return isinstance(o, K) and self.v == o.v


# Set equality with distinct-but-equal nested objects.
print({('x', K(1))} == {('x', K(1))})            # True

# Tuple-key membership in a set.
print(('x', K(1)) in {('x', K(1))})              # True

# Dict subscript / get with an equal-but-distinct nested object.
d = {(1, K(1)): 'v'}
print(d[(1, K(1))])                              # v
print(d.get((1, K(1))))                          # v
print((1, K(1)) in d)                            # True

# Dict-literal dedup: last value wins, one entry.
d2 = {(1, K(1)): 'a', (1, K(1)): 'b'}
print(len(d2), list(d2.values()))               # 1 ['b']

# Set-literal dedup.
print(len({(1, K(1)), (1, K(1))}))              # 1

# Frozenset with nested object.
print(frozenset({(1, K(1))}) == frozenset({(1, K(1))}))  # True

# Deeper nesting (tuple in tuple).
print((1, (2, K(3))) in {(1, (2, K(3)))})       # True

# Negative: unequal nested objects must NOT match.
print((1, K(1)) in {(1, K(2))})                 # False
print(len({(1, K(1)), (1, K(2))}))              # 2

# Set algebra over nested-object tuple keys.
print({(1, K(1)), (1, K(2))} & {(1, K(1))} == {(1, K(1))})  # True
print({(1, K(1))} | {(1, K(1))} == {(1, K(1))})             # True
print({(1, K(1)), (1, K(2))} - {(1, K(1))} == {(1, K(2))})  # True

# Primitive-tuple keys remain unaffected (fast path).
print((1, 2) in {(1, 2), (3, 4)})               # True
print({(1, 2): 'a'}[(1, 2)])                    # a
print(len({(1, 2), (1, 2), (3, 4)}))            # 2
