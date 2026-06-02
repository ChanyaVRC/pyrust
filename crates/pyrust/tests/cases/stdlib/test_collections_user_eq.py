# Parity fixture for issue #1919: collections.Counter / defaultdict dispatch
# user __eq__ for keys.
#
# Counter and defaultdict store entries in a dict; their key get/insert/
# contains paths must route through the same __eq__-dispatch the builtin dict
# uses, so lookups hit, counts accumulate, and no two __eq__-equal keys are
# stored.  Primitive-key stores keep the fast path.

import collections


class E:
    def __init__(self, v):
        self.v = v

    def __eq__(self, o):
        return isinstance(o, E) and self.v == o.v

    def __hash__(self):
        return hash(self.v)


# defaultdict: lookup hits an equal key, no duplicate stored.
dd = collections.defaultdict(int)
dd[E(1)] = 5
print(dd[E(1)])          # 5
print(E(1) in dd)        # True
dd[E(1)] = 9
print(len(dd))           # 1
print(dd.get(E(1)))      # 9

# defaultdict factory + accumulation across equal keys.
dd2 = collections.defaultdict(int)
dd2[E(7)] += 1
dd2[E(7)] += 1
print(dd2[E(7)], len(dd2))   # 2 1

# Counter(iterable) accumulates over equal elements.
c = collections.Counter([E(1), E(1), E(2)])
print(sorted((k.v, n) for k, n in c.items()))   # [(1, 2), (2, 1)]
print(c[E(1)])           # 2
print(E(1) in c)         # True

# Counter[k] += 1 accumulates.
c2 = collections.Counter()
c2[E(1)] += 1
c2[E(1)] += 1
print(c2[E(1)], len(c2))     # 2 1

# Counter.update with equal keys.
c3 = collections.Counter([E(1)])
c3.update([E(1), E(2)])
print(sorted((k.v, n) for k, n in c3.items()))  # [(1, 2), (2, 1)]

# Counter arithmetic merges equal keys (multiset add).
ca = collections.Counter([E(1), E(1), E(2)])
cb = collections.Counter([E(1), E(3)])
print(sorted((k.v, n) for k, n in (ca + cb).items()))  # [(1, 3), (2, 1), (3, 1)]

# Missing key returns 0 (Counter) / triggers factory (defaultdict).
print(collections.Counter()[E(99)])   # 0
print(collections.defaultdict(int)[E(99)])  # 0

# Primitive-key Counters / defaultdicts are unaffected.
pc = collections.Counter([1, 1, 2, 3, 3, 3])
print(sorted(pc.items()))                 # [(1, 2), (2, 1), (3, 3)]
pdd = collections.defaultdict(int)
pdd[5] += 1
pdd[5] += 1
print(pdd[5], len(pdd))                   # 2 1
