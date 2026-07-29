# The empty tuple is a shared singleton (CPython parity): `() is ()` is True and
# an empty `*args` collection costs no allocation.
print(() is ())
print(tuple() is ())
print(id(()) == id(()))
a = ()
b = ()
print(a is b, a == b, len(a), bool(a))

def f(*args):
    return args
print(f() is ())          # empty *args is the singleton
print(f() is f())
print(f(1) == (1,), f(1, 2) == (1, 2))   # non-empty still works

# empty tuple in common producers
print(tuple([]) is (), tuple("") is (), tuple(range(0)) is ())
print(().__class__ is tuple, isinstance((), tuple))
# hashing / dict key
d = {(): "empty"}
print(d[()], d[tuple()])
