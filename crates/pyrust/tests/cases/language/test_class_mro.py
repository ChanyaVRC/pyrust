# class.__mro__ — single-inheritance method resolution order (#367)
#
# pyrust's `PyClass` only carries a single `base` pointer, so this
# script exercises the single-inheritance MRO path. The terminal entry
# is always the synthetic `object` class, matching CPython's
# `cls.__mro__[-1] is object`.

class A:
    pass

class B(A):
    pass

print([c.__name__ for c in B.__mro__])  # ['B', 'A', 'object']

class C:
    pass

print([c.__name__ for c in C.__mro__])  # ['C', 'object']

# `__mro__` is a tuple, not a list.
print(type(B.__mro__).__name__)         # 'tuple'

# Deeper chain.
class D(B):
    pass

print([c.__name__ for c in D.__mro__])  # ['D', 'B', 'A', 'object']

# The first entry is the class itself; the last is `object`.
print(B.__mro__[0].__name__)            # 'B'
print(B.__mro__[-1].__name__)           # 'object'

# `object` is shared across MROs (identity, not just equality).
print(A.__mro__[-1] is B.__mro__[-1])   # True
print(C.__mro__[-1] is D.__mro__[-1])   # True
