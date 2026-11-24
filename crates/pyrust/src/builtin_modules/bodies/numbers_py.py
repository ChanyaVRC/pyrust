"""Abstract Base Classes (ABCs) for numbers, according to PEP 3141.

Port of CPython 3.12's ``Lib/numbers.py``.
``ABCMeta`` and ``abstractmethod`` are pre-seeded into the exec namespace by
``numbers.rs::inject_python_members``.
"""

__all__ = ["Number", "Complex", "Real", "Rational", "Integral"]


class Number(metaclass=ABCMeta):
    """All numbers inherit from this class.

    If you just want to check if an argument x is a number, without
    caring what kind, use isinstance(x, Number).
    """

    __slots__ = ()

    # Concrete numeric types must provide their own hash implementation
    __hash__ = None


class Complex(Number):
    """Complex defines the operations that work on the builtin complex type.

    In short, those are: a conversion to complex, .real, .imag, +, -,
    *, /, **, abs(), .conjugate, ==, and !=.

    If it is given heterogeneous arguments, and doesn't have special
    knowledge about them, it should fall back to the builtin complex
    type as described below.
    """

    __slots__ = ()

    @abstractmethod
    def __complex__(self):
        """Return a builtin complex instance. Called for complex(self)."""

    def __bool__(self):
        """True if self != 0. Called for bool(self)."""
        return self != 0

    @property
    @abstractmethod
    def real(self):
        """Retrieve the real component of this number.

        This should subclass Real.
        """
        raise NotImplementedError

    @property
    @abstractmethod
    def imag(self):
        """Retrieve the imaginary component of this number.

        This should subclass Real.
        """
        raise NotImplementedError

    @abstractmethod
    def __add__(self, other):
        """self + other"""
        raise NotImplementedError

    @abstractmethod
    def __radd__(self, other):
        """other + self"""
        raise NotImplementedError

    @abstractmethod
    def __neg__(self):
        """-self"""
        raise NotImplementedError

    @abstractmethod
    def __pos__(self):
        """+self"""
        raise NotImplementedError

    def __sub__(self, other):
        """self - other"""
        return self + -other

    def __rsub__(self, other):
        """other - self"""
        return -self + other

    @abstractmethod
    def __mul__(self, other):
        """self * other"""
        raise NotImplementedError

    @abstractmethod
    def __rmul__(self, other):
        """other * self"""
        raise NotImplementedError

    @abstractmethod
    def __truediv__(self, other):
        """self / other: Should promote to float when necessary."""
        raise NotImplementedError

    @abstractmethod
    def __rtruediv__(self, other):
        """other / self"""
        raise NotImplementedError

    @abstractmethod
    def __pow__(self, exponent):
        """self ** exponent; should promote to float or complex when necessary."""
        raise NotImplementedError

    @abstractmethod
    def __rpow__(self, base):
        """base ** self"""
        raise NotImplementedError

    @abstractmethod
    def __abs__(self):
        """Returns the Real distance from 0. Called for abs(self)."""
        raise NotImplementedError

    @abstractmethod
    def conjugate(self):
        """(x+y*i).conjugate() returns (x-y*i)."""
        raise NotImplementedError

    @abstractmethod
    def __eq__(self, other):
        """self == other"""
        raise NotImplementedError


Complex.register(complex)


class Real(Complex):
    """To Complex, Real adds the operations that work on real numbers.

    In short, those are: a conversion to float, trunc(), divmod,
    %, <, <=, >, and >=.

    Real also provides defaults for the derived operations.
    """

    __slots__ = ()

    @abstractmethod
    def __float__(self):
        """Any Real can be converted to a native float object.

        Called for float(self)."""
        raise NotImplementedError

    @abstractmethod
    def __trunc__(self):
        """trunc(self): Truncates self to an Integral."""
        raise NotImplementedError

    @abstractmethod
    def __floor__(self):
        """Finds the greatest Integral <= self."""
        raise NotImplementedError

    @abstractmethod
    def __ceil__(self):
        """Finds the least Integral >= self."""
        raise NotImplementedError

    @abstractmethod
    def __round__(self, ndigits=None):
        """Rounds self to ndigits decimal places, defaulting to 0.

        If ndigits is omitted or None, returns an Integral, otherwise
        returns a Real. Rounds half toward even.
        """
        raise NotImplementedError

    def __divmod__(self, other):
        """divmod(self, other): The pair (self // other, self % other)."""
        return (self // other, self % other)

    def __rdivmod__(self, other):
        """divmod(other, self): The pair (other // self, other % self)."""
        return (other // self, other % self)

    @abstractmethod
    def __floordiv__(self, other):
        """self // other: The floor() of self/other."""
        raise NotImplementedError

    @abstractmethod
    def __rfloordiv__(self, other):
        """other // self: The floor() of other/self."""
        raise NotImplementedError

    @abstractmethod
    def __mod__(self, other):
        """self % other"""
        raise NotImplementedError

    @abstractmethod
    def __rmod__(self, other):
        """other % self"""
        raise NotImplementedError

    @abstractmethod
    def __lt__(self, other):
        """self < other

        < on Reals defines a total ordering, except perhaps for NaN."""
        raise NotImplementedError

    @abstractmethod
    def __le__(self, other):
        """self <= other"""
        raise NotImplementedError

    # Concrete implementations of Complex abstract methods.
    def __complex__(self):
        """complex(self) == complex(float(self), 0)"""
        return complex(float(self))

    @property
    def real(self):
        """Real numbers are their real component."""
        return +self

    @property
    def imag(self):
        """Real numbers have no imaginary component."""
        return 0

    def conjugate(self):
        """Conjugate is a no-op for Reals."""
        return +self


Real.register(float)


class Rational(Real):
    """To Real, Rational adds numerator and denominator properties.

    The numerator and denominator values should be in lowest terms,
    with a positive denominator.
    """

    __slots__ = ()

    @property
    @abstractmethod
    def numerator(self):
        """The numerator of a rational number in lowest terms."""
        raise NotImplementedError

    @property
    @abstractmethod
    def denominator(self):
        """The denominator of a rational number in lowest terms.

        This denominator should be positive.
        """
        raise NotImplementedError

    # Concrete implementation of Real's conversion to float.
    def __float__(self):
        """float(self) = self.numerator / self.denominator

        It's important that this conversion use the integer's "true"
        division rather than casting one side to float before dividing
        so that ratios of huge integers convert without overflowing.
        """
        return int(self.numerator) / int(self.denominator)


class Integral(Rational):
    """Integral adds methods that work on integral numbers.

    In short, these are conversion to int, pow with modulus, and the
    bit-string operations.
    """

    __slots__ = ()

    @abstractmethod
    def __int__(self):
        """int(self)"""
        raise NotImplementedError

    def __index__(self):
        """Called whenever an index is needed, such as in slicing"""
        return int(self)

    @abstractmethod
    def __pow__(self, exponent, modulus=None):
        """self ** exponent % modulus, but maybe faster."""
        raise NotImplementedError

    @abstractmethod
    def __lshift__(self, other):
        """self << other"""
        raise NotImplementedError

    @abstractmethod
    def __rlshift__(self, other):
        """other << self"""
        raise NotImplementedError

    @abstractmethod
    def __rshift__(self, other):
        """self >> other"""
        raise NotImplementedError

    @abstractmethod
    def __rrshift__(self, other):
        """other >> self"""
        raise NotImplementedError

    @abstractmethod
    def __and__(self, other):
        """self & other"""
        raise NotImplementedError

    @abstractmethod
    def __rand__(self, other):
        """other & self"""
        raise NotImplementedError

    @abstractmethod
    def __xor__(self, other):
        """self ^ other"""
        raise NotImplementedError

    @abstractmethod
    def __rxor__(self, other):
        """other ^ self"""
        raise NotImplementedError

    @abstractmethod
    def __or__(self, other):
        """self | other"""
        raise NotImplementedError

    @abstractmethod
    def __ror__(self, other):
        """other | self"""
        raise NotImplementedError

    @abstractmethod
    def __invert__(self):
        """~self"""
        raise NotImplementedError

    # Concrete implementations of Rational and Real abstract methods.
    def __float__(self):
        """float(self) == float(int(self))"""
        return float(int(self))

    @property
    def numerator(self):
        """Integers are their own numerators."""
        return +self

    @property
    def denominator(self):
        """Integers have a denominator of 1."""
        return 1


Integral.register(int)
