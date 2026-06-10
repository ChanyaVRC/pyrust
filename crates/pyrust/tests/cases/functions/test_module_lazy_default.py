# f.__module__ lazy default (#2256): the "__main__" default is materialised on
# read from an `unset` sentinel rather than stored per-closure, but the
# observable behaviour must match CPython exactly — default is "__main__",
# assignment sticks, `del` resets to None, and each closure of a shared `def`
# carries its own independent __module__ / __name__ even though the immutable
# name/qualname allocation is shared.


def f():
    pass


print(f.__module__)  # __main__

f.__module__ = "custom.mod"
print(f.__module__)  # custom.mod

f.__module__ = 123  # any value is allowed
print(f.__module__)  # 123

del f.__module__
print(f.__module__)  # None  (del leaves None, not the default)


# A second, untouched function still reports the default — no shared module cell.
def g():
    pass


print(g.__module__)  # __main__


# Closures: every instance of one `def` shares the immutable name/qualname but
# has independent mutable dunders.
def make(tag):
    def inner():
        return tag

    return inner


a = make(1)
b = make(2)
print(a.__name__, b.__name__)  # inner inner
print(a.__qualname__, b.__qualname__)  # make.<locals>.inner make.<locals>.inner
print(a.__module__, b.__module__)  # __main__ __main__

a.__module__ = "A"
a.__name__ = "renamed_a"
# Mutating one closure must not affect its sibling.
print(a.__module__, a.__name__)  # A renamed_a
print(b.__module__, b.__name__)  # __main__ inner
print(a() == 1, b() == 2)  # True True


# __module__ participates in repr-independent introspection alongside __doc__.
def documented():
    "a docstring"
    return 0


print(documented.__module__, documented.__doc__)  # __main__ a docstring
