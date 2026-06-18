# Issue #2544: complex subclass instances must behave as numbers.
# Covers .real/.imag/conjugate descriptors, arithmetic (both operand sides),
# unbound slot-method dispatch with a subclass receiver, and that a user
# arithmetic override still wins over the inherited complex slot.


class C(complex):
    pass


c = C(1, 2)

# Read-only numeric-tower attributes resolve through the complex backing.
print(c.real)
print(c.imag)
print(type(c.real).__name__, type(c.imag).__name__)
print(c.conjugate())
print(type(c.conjugate()).__name__)

# Arithmetic: subclass on the left and on the right.
print(c + 1)
print(1 + c)
print(c - 1)
print(c * 2)
print(c / 2)
print(c ** 2)
print(-c)
print(abs(c))
print(2 - c)
print(2 / c)

# complex + complex-subclass and subclass + subclass.
print(c + complex(0, 1))
print(C(0, 1) + C(1, 0))

# Result of inherited arithmetic is a plain complex, not the subclass.
print(type(c + 1).__name__)
print(type(c).__name__)

# Equality with a plain complex.
print(c == complex(1, 2))

# Unbound slot methods accept the subclass instance as receiver.
print(C.__add__(c, 1))
print(C.__mul__(c, 2))
print(C.__abs__(c))
print(complex.__abs__(c))


# A user arithmetic override wins over the inherited complex slot.
class D(complex):
    def __add__(self, other):
        return "custom"


print(D(1, 2) + 1)

# repr/str of a complex subclass instance.
print(repr(c))
print(str(c))

# Operators complex does not support raise TypeError naming the *subclass*
# type (not the coerced base `complex`), matching CPython (issue #2544).
for expr in ("c // 1", "c % 1", "c / 'x'", "c ** 'x'"):
    try:
        eval(expr)
    except TypeError as e:
        print(e)
