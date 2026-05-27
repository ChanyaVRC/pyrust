# Tests for container subclass __repr__ / __str__ inheritance (issue #1205).
# CPython 3.12 rule:
#   - list/dict/tuple subclasses inherit __repr__ that renders the container
#     contents (same format as the base type).
#   - set/frozenset subclasses prefix the class name:
#     `ClassName({element, ...})` or `ClassName()` for empty.
# Subclasses that define their own __repr__ or __str__ use that instead.

class MyList(list): pass
class MyDict(dict): pass
class MyTuple(tuple): pass
class MySet(set): pass
class MyFrozenset(frozenset): pass

# Basic repr
print(repr(MyList([1, 2, 3])))           # [1, 2, 3]
print(repr(MyDict({"a": 1})))             # {'a': 1}
print(repr(MyTuple((1, 2))))              # (1, 2)
# set order is not guaranteed; just check the class name prefix is present
s = repr(MySet({1}))
print(s.startswith("MySet("))             # True
print(repr(MyFrozenset({1})).startswith("MyFrozenset("))  # True

# Empty containers
print(repr(MyList()))                     # []
print(repr(MyDict()))                     # {}
print(repr(MyTuple()))                    # ()
print(repr(MySet()))                      # MySet()
print(repr(MyFrozenset()))                # MyFrozenset()

# str() should produce the same result as repr() for containers
print(str(MyList([1, 2, 3])))            # [1, 2, 3]
print(str(MyDict({"a": 1})))              # {'a': 1}
print(str(MySet({1})) == repr(MySet({1})))  # True

# f-string !r and !s conversions
x = MyList([1, 2])
print(f"{x!r}")                            # [1, 2]
print(f"{x!s}")                            # [1, 2]

# Custom __repr__ overrides the inherited one
class MyList2(list):
    def __repr__(self): return "custom_repr"

print(repr(MyList2([1, 2, 3])))           # custom_repr

# Custom __str__ overrides str() but not repr()
class MyList3(list):
    def __str__(self): return "custom_str"

print(str(MyList3([1, 2, 3])))            # custom_str
print(repr(MyList3([1, 2, 3])))           # [1, 2, 3]

# Nested subclass instances
class MyList4(list): pass
print(repr(MyList4([MyList4([1, 2]), 3])))  # [[1, 2], 3]

# print() uses str(), which should match repr() for container subclasses
print(MyList([1, 2, 3]))                  # [1, 2, 3]
print(MyDict({"a": 1}))                    # {'a': 1}
