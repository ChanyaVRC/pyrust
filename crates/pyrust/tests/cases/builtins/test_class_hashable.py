"""Parity fixture: class objects are hashable by identity (issue #1189).

CPython: all class objects (user-defined and built-in) are hashable by
identity.  hash(Foo) returns a stable integer and classes can be used as
dict keys or set members.
"""

class Foo:
    pass


class Bar:
    pass


# Class objects are hashable; hash() returns an int
h = hash(Foo)
print(type(h).__name__)   # int

# Consistent hash (same class -> same hash within a run)
print(hash(Foo) == hash(Foo))   # True

# Distinct classes produce distinct hashes (pointer-based, no collision)
print(hash(Foo) == hash(Bar))   # False

# Use class as dict key
d = {Foo: 1, Bar: 2}
print(d[Foo])   # 1
print(d[Bar])   # 2

# Use class in set
s = {Foo, Bar, int, str}
print(len(s))   # 4
print(Foo in s)  # True
print(int in s)  # True

# Built-in classes are also hashable
print(type(hash(int)).__name__)    # int
print(type(hash(str)).__name__)    # int
print(int in {int, str})           # True
