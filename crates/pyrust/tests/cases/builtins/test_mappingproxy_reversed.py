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


# An empty class still produces a valid (possibly non-empty, dunder-only)
# reverse iterator that round-trips against its own keys.
class Empty:
    pass


print(list(reversed(vars(Empty))) == list(reversed(list(vars(Empty).keys()))))  # True
