class Foo:
    x = 1
    y = 2


# reversed(vars(C)) returns the keys in reverse insertion order.
# CPython's exact key set (which dunders appear) is implementation-defined,
# so compare against the reverse of the proxy's own forward key order rather
# than hard-coding a literal list.
keys = list(reversed(vars(Foo)))
print(keys == list(reversed(list(vars(Foo).keys()))))  # True

# Forward iteration is unaffected.
print(list(vars(Foo)) == list(vars(Foo).keys()))  # True

# The __reversed__ dunder is directly callable and matches reversed().
print(list(vars(Foo).__reversed__()) == list(reversed(vars(Foo))))  # True

# Extra arguments to __reversed__ raise TypeError.
try:
    vars(Foo).__reversed__(1)
except TypeError:
    print("TypeError")

# Keyword arguments to __reversed__ also raise TypeError (checked before
# positional-arg arity, matching CPython 3.12).
try:
    vars(Foo).__reversed__(x=1)
except TypeError as e:
    print(str(e))


# An empty class still produces a valid (possibly non-empty, dunder-only)
# reverse iterator that round-trips against its own keys.
class Empty:
    pass


print(list(reversed(vars(Empty))) == list(reversed(list(vars(Empty).keys()))))  # True


# Dict-backed mappingproxy (issue #2679) also supports reversed().
d = {"a": 1, "b": 2, "c": 3}
m = d.keys().mapping
print(list(reversed(m)) == list(reversed(list(m.keys()))))  # True
print(list(m.__reversed__()) == list(reversed(m)))  # True

# Verify list(reversed(m)) matches list(reversed(list(m.keys()))) for
# both empty and non-empty dict-backed proxies (issue #2684).
empty_proxy = {}.keys().mapping
print(list(reversed(empty_proxy)) == [])  # True
