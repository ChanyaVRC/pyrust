d = {"a": 1, "b": 2, "c": 3}

# get
assert d.get("a") == 1
assert d.get("z") is None
assert d.get("z", 99) == 99

# keys / values / items
assert list(d.keys()) == ["a", "b", "c"]
assert list(d.values()) == [1, 2, 3]
assert list(d.items()) == [("a", 1), ("b", 2), ("c", 3)]

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
