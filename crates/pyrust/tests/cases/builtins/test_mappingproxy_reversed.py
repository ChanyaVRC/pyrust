class Foo:
    x = 1
    y = 2

# reversed() returns keys in reverse insertion order.
# Compare against the reverse of its own forward key order (implementation-defined dunders).
keys = list(reversed(vars(Foo)))
print(keys == list(reversed(list(vars(Foo).keys()))))  # True
print(list(vars(Foo)) == list(vars(Foo).keys()))  # True (forward unaffected)
print(list(vars(Foo).__reversed__()) == list(reversed(vars(Foo))))  # True

try:
    vars(Foo).__reversed__(1)
except TypeError:
    print("TypeError")

try:
    vars(Foo).__reversed__(x=1)
except TypeError as e:
    print(str(e))

class Empty:
    pass
print(list(reversed(vars(Empty))) == list(reversed(list(vars(Empty).keys()))))  # True

# Dict-backed variant (issue #2679)
d = {"a": 1, "b": 2, "c": 3}
m = d.keys().mapping
print(list(reversed(m)) == list(reversed(list(m.keys()))))  # True
print(list(m.__reversed__()) == list(reversed(m)))  # True
print(list(reversed({}.keys().mapping)) == [])  # True
