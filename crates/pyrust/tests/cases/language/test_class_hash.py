"""Parity fixture: class objects are hashable by identity (issue #1189).

CPython: all class objects (user-defined and built-in) are hashable.
hash(Foo) returns a stable integer; class objects can be dict/set keys.
"""

class Foo:
    pass


class Bar:
    pass


# hash() returns an int
print(type(hash(Foo)).__name__)

# Same class hashes to the same value within a run
print(hash(Foo) == hash(Foo))

# Two distinct classes have (almost certainly) different hashes —
# but we only test that each is an int; pointer collision is impossible
# to guarantee and would make the fixture fragile.
print(type(hash(Bar)).__name__)

# Class objects work as dict keys
d = {int: "integer", str: "string", Foo: "foo", Bar: "bar"}
print(len(d))
print(d[int])
print(d[str])
print(d[Foo])
print(d[Bar])

# Class objects work as set members
s = {Foo, int, str}
print(len(s))
print(int in s)
print(str in s)
print(Foo in s)
print(Bar in s)

# Lookup round-trips correctly
d2 = {Foo: 42}
print(d2[Foo])

# Built-in classes
d3 = {int: 1, float: 2, bool: 3, list: 4, dict: 5, set: 6, tuple: 7, str: 8}
print(len(d3))
print(d3[int])
print(d3[str])

# Classes in sets deduplicate properly (same class == same key)
s2 = {int, int, int}
print(len(s2))
