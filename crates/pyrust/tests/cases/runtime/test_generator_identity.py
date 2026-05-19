# Generator identity: g is iter(g) must be True (issue #714).
#
# CPython guarantees that calling iter() on a generator object returns the
# same generator unchanged.  Identity comparison (is) and id() must both
# use the Rc pointer address rather than returning False / 0.


def gen():
    yield 1
    yield 2


# iter() on a generator returns the same object (not a copy).
g = gen()
print(g is iter(g))       # True
print(g is not iter(g))   # False

# Self-identity is always True.
g2 = gen()
print(g2 is g2)           # True

# Two distinct generator objects are not identical.
g3 = gen()
g4 = gen()
print(g3 is g4)           # False
print(g3 is not g4)       # True

# id() must be non-zero and stable across multiple calls.
g5 = gen()
id_a = id(g5)
id_b = id(g5)
print(id_a == id_b)       # True
print(id_a != 0)          # True

# id() of two distinct generators must differ.
g6 = gen()
print(id(g5) != id(g6))   # True

# Aliasing: id of alias equals id of original.
alias = g5
print(id(alias) == id(g5))  # True
print(alias is g5)           # True

# An exhausted generator retains its identity.
def gen_one():
    yield 42

ex = gen_one()
next(ex)
try:
    next(ex)
except StopIteration:
    pass
print(ex is iter(ex))     # True
