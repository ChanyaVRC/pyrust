d = {"a": 1, "b": 2, "c": 3}

# get
assert d.get("a") == 1
assert d.get("z") is None
assert d.get("z", 99) == 99

# keys / values / items
assert list(d.keys()) == ["a", "b", "c"]
assert list(d.values()) == [1, 2, 3]
assert list(d.items()) == [("a", 1), ("b", 2), ("c", 3)]

# Iterating dict views (regression: the iter_values BuiltinObject arm
# in expr.rs replaced an unwrap with a structured TypeError fallback;
# verify the happy path through that branch still works).
collected_keys = []
for k in d.keys():
    collected_keys.append(k)
assert collected_keys == ["a", "b", "c"]

collected_vals = []
for v in d.values():
    collected_vals.append(v)
assert collected_vals == [1, 2, 3]

collected_items = []
for kv in d.items():
    collected_items.append(kv)
assert collected_items == [("a", 1), ("b", 2), ("c", 3)]

# Splat-unpack a dict view through call args (also routes through iter_values).
def _three(a, b, c):
    return (a, b, c)
assert _three(*d.keys()) == ("a", "b", "c")
assert _three(*d.values()) == (1, 2, 3)

# len() on dict views — dispatches through BuiltinTypeOps::len.
assert len(d.keys()) == 3
assert len(d.values()) == 3
assert len(d.items()) == 3

# `in` on dict views — dispatches through BuiltinTypeOps::contains.
assert "a" in d.keys()
assert "z" not in d.keys()
assert 1 in d.values()
assert 99 not in d.values()
assert ("a", 1) in d.items()
assert ("a", 2) not in d.items()

# update
d.update({"d": 4})
assert d["d"] == 4

# pop
v = d.pop("d")
assert v == 4
assert "d" not in d

# pop with default
v2 = d.pop("z", 0)
assert v2 == 0

# popitem (last inserted = "c")
k, v = d.popitem()
assert k == "c" and v == 3

# setdefault
d.setdefault("e", 5)
assert d["e"] == 5
d.setdefault("a", 99)
assert d["a"] == 1  # already exists, not overwritten

# copy
e = d.copy()
e["x"] = 100
assert "x" not in d

# clear
f = {"q": 1}
f.clear()
assert f == {}

print("dict methods OK")
