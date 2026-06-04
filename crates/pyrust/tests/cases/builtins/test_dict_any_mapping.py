# dict() / {**m} accept any keys()-bearing mapping (issue #2190)
# CPython 3.12 parity: ChainMap / UserDict / OrderedDict / a custom class
# exposing keys() + __getitem__ are all materialised via the mapping protocol.
import collections

# ChainMap: first-map-wins, later maps fill in remaining keys.
c = collections.ChainMap({"a": 1, "x": 10}, {"a": 2, "b": 3})
print(dict(c))
print({**c})
print(dict(c.new_child({"q": 9})))
print(dict(c.parents))

# Empty ChainMap and empty maps.
print(dict(collections.ChainMap()))
print({**collections.ChainMap({}, {})})

# UserDict and OrderedDict.
print(dict(collections.UserDict({"p": 1, "q": 2})))
print({**collections.UserDict({"p": 1, "q": 2})})
print(dict(collections.OrderedDict([("z", 1), ("y", 2)])))

# A custom duck-typed mapping (keys() + __getitem__).
class M:
    def keys(self):
        return ["k", "j"]

    def __getitem__(self, key):
        return key.upper()

print(dict(M()))
print({**M()})

# kwargs combine with a mapping positional.
print(dict(collections.ChainMap({"a": 1}), b=2))

# Existing iterable-of-pairs and dict(dict) paths are unregressed.
print(dict([("a", 1), ("b", 2)]))
print(dict({"a": 1}))
print(dict(iter([("c", 3)])))

# Merge ** with literal keys.
d1 = {"x": 1}
d2 = {"y": 2}
print({**d1, **d2, "z": 3})
