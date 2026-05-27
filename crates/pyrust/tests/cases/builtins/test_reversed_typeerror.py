# Parity fixture: reversed() raises TypeError for non-reversible objects,
# and produces the correct type name in the error message.
# Issue #1283 — CPython 3.12 reference.

# --- Objects that must raise TypeError ---

# Generator expression
try:
    reversed(x for x in range(3))
    print("FAIL: generator should raise TypeError")
except TypeError as e:
    print(str(e))

# list_iterator (iter on a list)
try:
    reversed(iter([1, 2, 3]))
    print("FAIL: list_iterator should raise TypeError")
except TypeError as e:
    print(str(e))

# tuple_iterator (iter on a tuple)
try:
    reversed(iter((1, 2, 3)))
    print("FAIL: tuple_iterator should raise TypeError")
except TypeError as e:
    print(str(e))

# set_iterator (iter on a set)
try:
    reversed(iter({1}))
    print("FAIL: set_iterator should raise TypeError")
except TypeError as e:
    print(str(e))

# dict_keyiterator (iter on a dict)
try:
    reversed(iter({"k": 1}))
    print("FAIL: dict_keyiterator should raise TypeError")
except TypeError as e:
    print(str(e))

# set (no __reversed__, no __getitem__)
try:
    reversed({1, 2, 3})
    print("FAIL: set should raise TypeError")
except TypeError as e:
    print(str(e))

# map object
try:
    reversed(map(str, [1, 2, 3]))
    print("FAIL: map should raise TypeError")
except TypeError as e:
    print(str(e))

# filter object
try:
    reversed(filter(None, [1, 2, 3]))
    print("FAIL: filter should raise TypeError")
except TypeError as e:
    print(str(e))

# User class with no __reversed__ and no __len__/__getitem__
class Irreversible:
    pass

try:
    reversed(Irreversible())
    print("FAIL: Irreversible should raise TypeError")
except TypeError as e:
    print(str(e))

# --- Objects that must work ---

# list (has __reversed__)
print(list(reversed([1, 2, 3])))

# tuple (has __len__ + __getitem__)
print(list(reversed((1, 2, 3))))

# range (has __reversed__)
print(list(reversed(range(5))))

# str (has __len__ + __getitem__)
print(list(reversed("abc")))

# bytes (has __len__ + __getitem__)
print(list(reversed(b"ab")))

# User class with __reversed__
class WithReversed:
    def __reversed__(self):
        return iter([30, 20, 10])

print(list(reversed(WithReversed())))

# User class with __len__ + __getitem__
class Sequence:
    def __len__(self):
        return 3
    def __getitem__(self, i):
        return [100, 200, 300][i]

print(list(reversed(Sequence())))
