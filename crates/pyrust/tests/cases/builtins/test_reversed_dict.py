# reversed() on dict and the three dict views (issue #2093)
# CPython 3.8+ reverses them by insertion order; the dunder is also exposed.

d = {"a": 1, "b": 2, "c": 3}
print(list(reversed(d)))
print(list(reversed(d.keys())))
print(list(reversed(d.values())))
print(list(reversed(d.items())))

# __reversed__ is exposed for both dict and the views.
print(hasattr(d, "__reversed__"))
print(hasattr(d.keys(), "__reversed__"))
print(hasattr(d.values(), "__reversed__"))
print(hasattr(d.items(), "__reversed__"))

# Calling the dunder directly works the same as reversed(...).
print(list(d.__reversed__()))
print(list(d.keys().__reversed__()))
print(list(d.values().__reversed__()))
print(list(d.items().__reversed__()))

# Empty dict and single-element dict.
print(list(reversed({})))
print(list(reversed({1: "x"})))

# OrderedDict (a dict subclass) reverses by insertion order too.
import collections

od = collections.OrderedDict([("x", 1), ("y", 2), ("z", 3)])
print(list(reversed(od)))

# Existing reversible builtins are unregressed.
print(list(reversed([1, 2, 3])))
print(list(reversed((1, 2, 3))))
print(list(reversed("abc")))
print(list(reversed(range(3))))
print(list(reversed(b"abc")))
print(list(reversed(bytearray(b"abc"))))

# A non-reversible builtin still raises TypeError.
try:
    reversed({1, 2, 3})
except TypeError as e:
    print(e)

# Passing an argument to the no-arg dunder raises the named-wrapper error.
for v in [d, d.keys(), d.values(), d.items()]:
    try:
        v.__reversed__(1)
    except TypeError as e:
        print(e)
