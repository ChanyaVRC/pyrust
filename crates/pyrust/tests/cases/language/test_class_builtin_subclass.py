# Parity fixture for issue #976: user classes that inherit from built-in
# container types (dict, list, set) should forward constructor args to the
# base type's initialiser and behave like the base type for subscript,
# method, and len() operations.

class MyDict(dict):
    pass

# Construction from a dict literal.
d = MyDict({"x": 10, "y": 20})
print(d["x"])            # 10
print(d["y"])            # 20

# isinstance and type checks.
print(isinstance(d, dict))   # True
print(isinstance(d, MyDict)) # True
print(type(d).__name__)      # MyDict

# Construction from keyword arguments.
d2 = MyDict(a=1, b=2)
print(d2["a"])           # 1
print(d2["b"])           # 2

# Construction from a list of pairs.
d3 = MyDict([("k", "v")])
print(d3["k"])           # v

# Item assignment on a dict subclass instance.
d["z"] = 99
print(d["z"])            # 99

# Dict methods via the backing dict.
print(sorted(d.keys()))  # ['x', 'y', 'z']
print(d.get("x"))        # 10
print(d.get("missing", 0))  # 0

# Empty construction.
d4 = MyDict()
print(len(d4))           # 0

# List subclass.
class MyList(list):
    pass

lst = MyList([1, 2, 3])
print(lst[0])            # 1
print(lst[2])            # 3
print(len(lst))          # 3

# isinstance and type checks for list subclass.
print(isinstance(lst, list))   # True
print(isinstance(lst, MyList)) # True
print(type(lst).__name__)      # MyList

# Item assignment on a list subclass instance.
lst[0] = 99
print(lst[0])            # 99

# List methods via the backing list.
lst.append(4)
print(len(lst))          # 4

# Set subclass.
class MySet(set):
    pass

s = MySet({1, 2, 3})
print(len(s))            # 3
print(isinstance(s, set))    # True
print(isinstance(s, MySet))  # True
print(type(s).__name__)      # MySet

# Set operations.
s.add(4)
print(len(s))            # 4

# Subclass with a custom __init__ that uses self[] and self.method
# inside a for loop (exercises the CallMethod inline cache fast path —
# regression for the bug where the second iteration failed with
# "builtin method not in registry").
class MyList2(list):
    def __init__(self, items):
        for item in items:
            self.append(item)

lst2 = MyList2([1, 2, 3])
print(len(lst2))         # 3
print(lst2[0])           # 1

class MyDict2(dict):
    def __init__(self, keys):
        for k in keys:
            self[k] = 1

d5 = MyDict2(["a", "b", "c"])
print(len(d5))           # 3
print(d5["a"])           # 1
