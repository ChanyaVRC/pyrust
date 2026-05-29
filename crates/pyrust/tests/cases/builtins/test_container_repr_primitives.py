# Parity fixture for repr() of containers holding only primitive elements.
# Issue #1282: render_value_repr unconditionally took the slow snapshot path
# for all containers; after the fix, containers of plain scalars use
# Value::repr() directly without allocating a snapshot vector.

# Lists of various primitive types
print(repr([]))
print(repr([1]))
print(repr([1, 2, 3]))
print(repr([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]))
print(repr(["hello", "world"]))
print(repr([1.5, 2.7]))
print(repr([True, False]))
print(repr([None, None]))
print(repr([1, "two", 3.0, True, None]))

# Tuples of various primitive types
print(repr(()))
print(repr((1,)))
print(repr((1, 2, 3)))
print(repr(("a", "b", "c")))
print(repr((1, 2.0, True, None)))

# Dicts with primitive keys and values
print(repr({}))
print(repr({1: 2}))
print(repr({1: 2, 3: 4}))
print(repr({"a": 1, "b": 2}))
print(repr({"key": "val", 1: None}))

# Sets of primitive types
print(repr(set()))
s1 = {1}
print(repr(s1))

# Frozensets of primitive types
print(repr(frozenset()))
fs1 = frozenset({1})
print(repr(fs1))

# Nested containers of primitives — inner containers go through fast path too
print(repr([[1, 2], [3, 4]]))
print(repr([(1, 2), (3, 4)]))
print(repr({"a": [1, 2], "b": [3, 4]}))

# Cycle detection still works — self-referential containers can't take the
# fast path because the inner element is a List/Dict (not a plain scalar)
a = []
a.append(a)
print(repr(a))

b = {}
b["self"] = b
print(repr(b))

# Mixed: user instance breaks fast path, primitives alongside are still repr'd
class Obj:
    def __repr__(self):
        return "Obj()"

obj = Obj()
print(repr([obj, 1, 2]))
print(repr((1, obj, 3)))
print(repr({1: obj}))
