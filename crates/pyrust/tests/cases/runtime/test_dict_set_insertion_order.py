# dict/set backing stores use a fast (FxHash) internal hasher instead of
# SipHash.  The hasher only changes bucket placement, never iteration order:
# dicts and sets must keep insertion order, and user __hash__/__eq__ keys
# must still dispatch correctly.  Version-stable: avoids printing raw set
# iteration order (CPython sets are hash-ordered) — uses dicts for ordering
# checks and sorted() for set contents.

# --- dict insertion order is preserved across build / mutate / re-insert ---
d = {}
for k in ["zebra", "apple", "mango", 5, 3, 1, -7]:
    d[k] = str(k).upper()
print(list(d))
print(list(d.keys()))
print(list(d.values()))
print(list(d.items()))

# dict literal + comprehension order
print({3: "a", 1: "b", 2: "c"})
print({i: i * i for i in range(6, 0, -1)})

# delete then re-insert moves the key to the end (insertion order)
d2 = {i: i for i in range(6)}
del d2[2]
d2[2] = 20
print(list(d2))

# update preserves existing position, appends new keys
d3 = {"a": 1, "b": 2}
d3.update({"c": 3, "a": 10})
print(list(d3.items()))

# --- set / frozenset content correctness (sorted for version stability) ---
a = {1, 2, 3, 4, 5, 6}
b = {4, 5, 6, 7, 8, 9}
print(sorted(a & b))
print(sorted(a | b))
print(sorted(a - b))
print(sorted(a ^ b))
print(sorted(frozenset([5, 1, 3, 9, 2])))

# set membership
s = set(range(0, 100, 7))
print(42 in s, 43 in s, 0 in s)
print(len(s))

# --- user __hash__/__eq__ object keys still dispatch correctly ---
class Key:
    def __init__(self, n):
        self.n = n
    def __hash__(self):
        return self.n % 3  # deliberate collisions
    def __eq__(self, other):
        return isinstance(other, Key) and self.n == other.n
    def __repr__(self):
        return f"K{self.n}"

m = {}
for i in range(6):
    m[Key(i)] = i
m[Key(2)] = 99  # overwrite via __eq__ match despite hash collisions
print(len(m))
print(m[Key(2)])
print([k.n for k in m])

ks = set()
for i in [1, 1, 2, 4, 4, 7]:
    ks.add(Key(i))
print(len(ks))
print(Key(4) in ks, Key(5) in ks)

# mixed primitive + object + None keys
mixed = {None: 0, True: 1, (1, 2): 2, "s": 3, Key(0): 4}
print(len(mixed))
print(mixed[Key(0)], mixed["s"], mixed[None])
print(list(mixed)[:4])
