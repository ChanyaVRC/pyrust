# Parity fixture: dict_keys / dict_items views are set-like (issue #1891).
# dict_values is NOT set-like.  Set results are sorted before printing so the
# output is deterministic across CPython and pyrust.

d = {"a": 1, "b": 2, "c": 3}

# --- dict_keys set operators (both operand orders) ---
print(sorted(d.keys() & {"a", "z"}))
print(sorted(d.keys() | {"x"}))
print(sorted(d.keys() - {"a"}))
print(sorted(d.keys() ^ {"a", "z"}))
print(sorted({"a", "z"} & d.keys()))
print(sorted({"x"} | d.keys()))
print(sorted({"a", "z"} - d.keys()))
print(sorted({"a", "z"} ^ d.keys()))

# Result type is always `set`, even with a frozenset operand.
print(type(d.keys() & {"a"}).__name__)
print(type(frozenset({"a"}) & d.keys()).__name__)
print(type(d.keys() | frozenset({"a"})).__name__)

# --- dict_items set operators ---
print(sorted({"a": 1}.items() & {("a", 1)}))
print(sorted(d.items() | {("z", 9)}))
print(sorted(d.items() - {("a", 1)}))

# --- equality (the silent-False bug) ---
print(d.keys() == {"a", "b", "c"})
print(d.keys() == frozenset({"a", "b", "c"}))
print(d.keys() == {"a": 9, "b": 9, "c": 9}.keys())
print({"a": 1}.items() == {("a", 1)})
print(d.items() == {("a", 1), ("b", 2), ("c", 3)})
print(d.keys() != {"a"})
print(d.keys() == {"a", "b"})

# A view is NOT equal to a non-set-like operand (returns False, not TypeError).
print(d.keys() == ["a", "b", "c"])
print(d.keys() == {"a": 1, "b": 2, "c": 3})
print(d.keys() == d.values())

# --- subset / superset comparisons ---
print({"a": 1}.keys() <= {"a", "b"})
print(d.keys() <= {"a", "b", "c", "d"})
print(d.keys() < {"a", "b", "c", "d"})
print(d.keys() >= {"a", "b"})
print(d.keys() > {"a", "b"})
print({"a"} <= d.keys())
print(d.keys() <= d.keys())

# --- isdisjoint (accepts any iterable) ---
print(d.keys().isdisjoint({"z"}))
print(d.keys().isdisjoint({"a"}))
print(d.keys().isdisjoint(["x", "y"]))
print(d.keys().isdisjoint("a"))
print(d.items().isdisjoint([("a", 1)]))
print(d.items().isdisjoint([("a", 99)]))
print(hasattr(d.keys(), "isdisjoint"))
print(hasattr(d.items(), "isdisjoint"))
print(hasattr(d.values(), "isdisjoint"))

# --- empty dict ---
e = {}
print(sorted(e.keys() & {1}))
print(e.keys() == set())
print(e.keys().isdisjoint([1, 2, 3]))

# --- dict_values is NOT set-like ---
for expr in ("vals & other", "vals | other", "vals == other", "other & vals"):
    try:
        vals = d.values()
        other = {1, 2, 3}
        eval(expr)
        print(expr, "no error")
    except TypeError as ex:
        print(expr, "->", ex)

# --- dict_items with unhashable values matches CPython ---
d2 = {"a": [1, 2]}
print(d2.items() == [("a", [1, 2])])   # other not set-like -> False (no raise)
print(d2.items() == {"a": [1, 2]})     # dict not set-like -> False
print(d2.items().isdisjoint([("a", 1)]))  # iterates other; works
for expr in ("d2.items() == {('a', 1)}", "d2.items() & {('a', 1)}", "d2.items() <= {('a', 1)}"):
    try:
        eval(expr)
        print(expr, "no error")
    except TypeError as ex:
        print(expr, "->", ex)

# --- isdisjoint argument-count / type errors ---
for expr in ("d.keys().isdisjoint()", "d.keys().isdisjoint(1, 2)", "d.keys().isdisjoint(5)"):
    try:
        eval(expr)
        print(expr, "no error")
    except TypeError as ex:
        print(expr, "->", ex)

# --- subset against non-set-like operand raises (not set-coerced) ---
try:
    d.keys() <= ["a", "b", "c"]
except TypeError as ex:
    print("keys <= list ->", ex)

# --- user-instance keys dispatch __eq__ through the view (issue #1907 machinery) ---
class K:
    def __init__(self, v):
        self.v = v

    def __hash__(self):
        return hash(self.v)

    def __eq__(self, other):
        return isinstance(other, K) and self.v == other.v


dk = {K(1): "a", K(2): "b"}
print(dk.keys() == {K(1), K(2)})
print(sorted((dk.keys() & {K(1), K(3)}), key=lambda k: k.v) == [K(1)])
