# Parity fixture for issue #1914: dict bulk ops dispatch user __eq__.
#
# dict.update, the | / |= merge operators, dict.fromkeys, and dict(pairs)
# must deduplicate keys via user __eq__ (last value wins on duplicates),
# instead of inserting two __eq__-equal keys.  Single-key ops already dedup;
# these fixtures cover the bulk paths.


class K:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return isinstance(o, K) and self.v == o.v

    def __hash__(self):
        return hash(self.v)


# update — last value wins.
d = {K(1): "a"}
d.update({K(1): "b", K(2): "c"})
print(sorted((k.v, v) for k, v in d.items()))   # [(1, 'b'), (2, 'c')]

# | merge — right operand wins on collision.
m = {K(1): "x"} | {K(1): "y", K(3): "z"}
print(sorted((k.v, v) for k, v in m.items()))   # [(1, 'y'), (3, 'z')]

# |= in-place merge.
ms = {K(1): "x"}
ms |= {K(1): "y"}
print(sorted((k.v, v) for k, v in ms.items()))  # [(1, 'y')]

# fromkeys — duplicate keys collapse, first-occurrence order preserved.
fk = dict.fromkeys([K(1), K(1), K(2)], 0)
print(len(fk))                                   # 2
print(sorted(k.v for k in fk))                   # [1, 2]

# dict(pairs) — iterable of (key, value), last value wins.
dp = dict([(K(1), 1), (K(1), 2)])
print(len(dp))                                   # 1
print(dp[K(1)])                                  # 2

# update from an iterable of pairs (non-dict source).
d2 = {K(5): "old"}
d2.update([(K(5), "new"), (K(6), "x")])
print(sorted((k.v, v) for k, v in d2.items()))   # [(5, 'new'), (6, 'x')]

# No resulting dict ever holds two __eq__-equal keys.
print(len({K(1): 1} | {K(1): 2} | {K(1): 3}))    # 1

# Single-key ops still behave (control).
si = {K(1): 1}
si[K(1)] = 2
print(len(si), si[K(1)])                          # 1 2

# Self-aliased update with object keys is a no-op (#448 + #1914).
sa = {K(1): "a", K(2): "b"}
sa.update(sa)
print(sorted((k.v, v) for k, v in sa.items()))   # [(1, 'a'), (2, 'b')]

# Primitive-key bulk ops are unaffected.
pd = {1: "a"}
pd.update({1: "b", 2: "c"})
print(sorted(pd.items()))                         # [(1, 'b'), (2, 'c')]
print(sorted(({1: 1} | {1: 2, 3: 3}).items()))   # [(1, 2), (3, 3)]
print(len(dict.fromkeys([1, 1, 2])))             # 2
