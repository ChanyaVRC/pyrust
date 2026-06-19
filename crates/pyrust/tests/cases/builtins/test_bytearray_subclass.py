# Issue #2386: bytearray subclass instances must inherit full bytearray
# behaviour. This fixture focuses on the augmented-assignment (`+=` / `*=`)
# in-place semantics, which mutate the backing bytearray and preserve the
# subclass type and object identity (matching CPython's bytearray.__iadd__ /
# __imul__, which mutate self and return self).


class BA(bytearray):
    pass


# `+=` mutates in place: same object, subclass type preserved, aliases updated.
c = BA(b"hi")
alias = c
before = id(c)
c += b"!"
print(c, type(c).__name__, id(c) == before, alias is c)

# `*=` mutates in place: same object, subclass type preserved.
d = BA(b"ab")
before = id(d)
d *= 2
print(d, type(d).__name__, id(d) == before)

# `*=` by zero / negative empties in place, keeping the subclass type.
e = BA(b"xy")
e *= 0
print(repr(e), type(e).__name__)
f = BA(b"xy")
f *= -3
print(repr(f), type(f).__name__)

# `*=` by a bool coerces to int.
g = BA(b"ab")
g *= True
print(g, type(g).__name__)

# `+=` accepts a bytes RHS and another bytearray subclass RHS.
h = BA(b"a")
h += b"bc"
print(h, type(h).__name__)
k = BA(b"a")
k += BA(b"z")
print(k, type(k).__name__)

# A plain bytearray `+=` a subclass RHS works and stays a plain bytearray.
m = bytearray(b"x")
m += BA(b"yz")
print(m, type(m).__name__)

# Bad RHS types raise the CPython-format TypeError, naming the LHS subclass.
try:
    n = BA(b"a")
    n += 5
except TypeError as ex:
    print("iadd-TE:", ex)

try:
    p = BA(b"a")
    p *= "x"
except TypeError as ex:
    print("imul-TE:", ex)

# A user-defined __iadd__ override still wins over the in-place fallback.
class BA2(bytearray):
    def __iadd__(self, other):
        return "overridden"


q = BA2(b"a")
q += b"b"
print(q)


# A user-defined __iadd__ / __imul__ that returns NotImplemented falls back to
# plain binary + / * (CPython drops the subclass type, yielding plain bytearray).
class BA3(bytearray):
    def __iadd__(self, other):
        return NotImplemented


u = BA3(b"a")
u += b"b"
print(u, type(u).__name__)


class BA4(bytearray):
    def __imul__(self, n):
        return NotImplemented


v = BA4(b"a")
v *= 2
print(v, type(v).__name__)

# Non-augmented binary `+` / `*` do not mutate the operand and follow the
# usual (non-subclass-preserving) bytearray result type.
a = BA(b"x")
r = a + BA(b"y")
print(a, r, type(r).__name__)
print(b"p" + BA(b"q"), type(b"p" + BA(b"q")).__name__)
print(BA(b"z") * 2, type(BA(b"z") * 2).__name__)
