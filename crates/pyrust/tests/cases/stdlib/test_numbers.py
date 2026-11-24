# CPython 3.12 parity tests for `numbers` ABCs (issue #3147).
import abc
import decimal
import fractions
import numbers
from decimal import Decimal
from fractions import Fraction
from numbers import Complex, Integral, Number, Rational, Real

# Module surface & metaclass
print("all:", numbers.__all__)
for cls in (Number, Complex, Real, Rational, Integral):
    print(cls.__name__, cls.__module__, type(cls) is abc.ABCMeta)

# Exact abstract method sets
for cls in (Number, Complex, Real, Rational, Integral):
    print("abstract", cls.__name__, sorted(cls.__abstractmethods__))

# Number itself has no abstract methods and can be instantiated;
# the other ABCs cannot be instantiated directly.
n_inst = Number()
print("Number() type:", type(n_inst).__name__, isinstance(n_inst, Number))
for cls in (Complex, Real, Rational, Integral):
    try:
        cls()
        print(cls.__name__, "unexpectedly instantiated")
    except TypeError:
        print(cls.__name__, "abstract TypeError ok")

# Number.__hash__ is None (concrete types must define __hash__)
print("Number.__hash__:", Number.__hash__)

# Full isinstance & issubclass matrix across builtin & stdlib numeric types
ABCS = (Number, Complex, Real, Rational, Integral)
VALUES = [
    ("bool_true", True),
    ("bool_false", False),
    ("int", 42),
    ("float", 1.5),
    ("complex", 1 + 2j),
    ("Fraction", Fraction(1, 2)),
    ("Decimal", Decimal("1.5")),
    ("str", "123"),
    ("None", None),
]
for label, val in VALUES:
    print("isinstance", label, [isinstance(val, a) for a in ABCS])

TYPES = [bool, int, float, complex, Fraction, Decimal, str, object]
for t in TYPES:
    print("issubclass", t.__name__, [issubclass(t, a) for a in ABCS])

# ABC hierarchy itself
for sub in ABCS:
    print("hierarchy", sub.__name__, [issubclass(sub, sup) for sup in ABCS])

# Fraction inherits from numbers.Rational and gains Real/Rational mixins
print("Fraction bases:", [b.__name__ for b in Fraction.__bases__])
f = Fraction(3, 4)
print("Fraction.real:", f.real, "imag:", f.imag, "conjugate:", f.conjugate())
print("float(Fraction):", float(f), "complex(Fraction):", complex(f))

# Cross-type comparisons enabled by real numbers ABCs in decimal & fractions
print("Decimal == Fraction:", Decimal("0.5") == Fraction(1, 2))
print("Fraction == Decimal:", Fraction(1, 2) == Decimal("0.5"))
print("Decimal < Fraction:", Decimal("0.5") < Fraction(2, 3))
print("Fraction > Decimal:", Fraction(2, 3) > Decimal("0.5"))
print("Decimal == complex:", Decimal("2") == 2 + 0j, Decimal("2") == 2 + 1j)
print("Fraction == complex:", Fraction(2, 1) == 2 + 0j, Fraction(2, 1) == 2 + 1j)


# Partial custom subclass of Real remains abstract
class IncompleteReal(Real):
    def __float__(self):
        return 0.0


try:
    IncompleteReal()
    print("IncompleteReal unexpectedly instantiated")
except TypeError:
    print("IncompleteReal abstract TypeError ok")


# Complete concrete custom Rational subclass exercising inherited mixins
class MyRational(Rational):
    def __init__(self, num, den=1):
        self._num = int(num)
        self._den = int(den)

    def __repr__(self):
        return f"MyRational({self._num}, {self._den})"

    @property
    def numerator(self):
        return self._num

    @property
    def denominator(self):
        return self._den

    def __add__(self, other):
        if isinstance(other, Rational):
            return MyRational(
                self._num * other.denominator + other.numerator * self._den,
                self._den * other.denominator,
            )
        return NotImplemented

    def __radd__(self, other):
        return self.__add__(other)

    def __neg__(self):
        return MyRational(-self._num, self._den)

    def __pos__(self):
        return MyRational(self._num, self._den)

    def __mul__(self, other):
        if isinstance(other, Rational):
            return MyRational(self._num * other.numerator, self._den * other.denominator)
        return NotImplemented

    def __rmul__(self, other):
        return self.__mul__(other)

    def __truediv__(self, other):
        if isinstance(other, Rational):
            return MyRational(self._num * other.denominator, self._den * other.numerator)
        return NotImplemented

    def __rtruediv__(self, other):
        if isinstance(other, Rational):
            return MyRational(other.numerator * self._den, other.denominator * self._num)
        return NotImplemented

    def __pow__(self, exponent):
        return float(self) ** exponent

    def __rpow__(self, base):
        return base ** float(self)

    def __abs__(self):
        return MyRational(abs(self._num), abs(self._den))

    def __eq__(self, other):
        if isinstance(other, Rational):
            return self._num * other.denominator == other.numerator * self._den
        return NotImplemented

    def __trunc__(self):
        return int(float(self))

    def __floor__(self):
        return self._num // self._den

    def __ceil__(self):
        return -(-self._num // self._den)

    def __round__(self, ndigits=None):
        return round(float(self), ndigits)

    def __floordiv__(self, other):
        return (self._num * other.denominator) // (self._den * other.numerator)

    def __rfloordiv__(self, other):
        return (other.numerator * self._den) // (other.denominator * self._num)

    def __mod__(self, other):
        return MyRational(
            (self._num * other.denominator) % (other.numerator * self._den),
            self._den * other.denominator,
        )

    def __rmod__(self, other):
        return MyRational(
            (other.numerator * self._den) % (self._num * other.denominator),
            self._den * other.denominator,
        )

    def __lt__(self, other):
        return self._num * other.denominator < other.numerator * self._den

    def __le__(self, other):
        return self._num * other.denominator <= other.numerator * self._den


mr = MyRational(3, 4)
print("MyRational abstract:", sorted(MyRational.__abstractmethods__))
print("MyRational isinstance:", [isinstance(mr, a) for a in ABCS])
print("MyRational float/complex/bool:", float(mr), complex(mr), bool(mr), bool(MyRational(0, 1)))
print("MyRational real/imag/conj:", float(mr.real), mr.imag, float(mr.conjugate()))
diff = mr - MyRational(1, 4)
print("MyRational sub mixin:", float(diff), float(MyRational(1, 1) - mr))
q, rem = divmod(MyRational(7, 2), MyRational(1, 1))
print("MyRational divmod mixin:", q, float(rem))

# Fraction interoperability with custom numbers.Rational
print("Fraction(MyRational):", Fraction(mr))
print("Fraction + MyRational:", Fraction(1, 4) + mr, mr + Fraction(1, 4))
print("Fraction == MyRational:", Fraction(3, 4) == mr, mr == Fraction(3, 4))
print("Decimal == MyRational:", Decimal("0.75") == mr)


# Virtual subclass registration at each level of the tower
class VNumber:
    pass


class VComplex:
    pass


class VReal:
    pass


class VRational:
    pass


class VIntegral:
    pass


class VIntegralSub(VIntegral):
    pass


Number.register(VNumber)
Complex.register(VComplex)
Real.register(VReal)
Rational.register(VRational)
Integral.register(VIntegral)

for cls in (VNumber, VComplex, VReal, VRational, VIntegral, VIntegralSub):
    print("virtual issubclass", cls.__name__, [issubclass(cls, a) for a in ABCS])
    print("virtual isinstance", cls.__name__, [isinstance(cls(), a) for a in ABCS])

# Registering a superclass as a virtual subclass must refuse to create a cycle
try:
    Integral.register(Real)
    print("cycle unexpectedly allowed")
except RuntimeError:
    print("cycle RuntimeError ok")

print("numbers ok")
