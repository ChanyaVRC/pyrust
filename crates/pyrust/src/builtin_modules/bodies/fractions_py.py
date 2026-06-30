# Fraction, infinite-precision, rational numbers.
#
# Adapted from CPython 3.12's Lib/fractions.py.  The reference module inherits
# from ``numbers.Rational`` and special-cases ``decimal.Decimal``; pyrust does
# not yet ship ``numbers`` or ``decimal``, so those dependencies are removed:
# ``Fraction`` is a plain class and the ``isinstance(x, numbers.Rational)`` /
# ``isinstance(x, Decimal)`` checks collapse to the concrete ``int`` / ``float``
# / ``Fraction`` cases pyrust supports.  All arithmetic, comparison, hashing,
# string parsing, ``limit_denominator`` and ``__format__`` logic is a faithful
# port.
#
# NOTE: ``functools``, ``math``, ``operator`` and ``re`` are NOT imported here.
# They are pre-seeded into this module's namespace from Rust
# (``fractions.rs::inject_python_members``) before this source is exec'd,
# because a top-level ``import X`` inside an explicit-dict exec namespace does
# not bind the name for module-top-level reads.

__all__ = ['Fraction']


# Constants related to the hash implementation;  hash(x) is based
# on the reduction of x modulo the prime _PyHASH_MODULUS.  CPython exposes
# these via ``sys.hash_info``; pyrust has no ``hash_info`` yet, so we hardcode
# the 64-bit CPython values (modulus = 2**61 - 1).
_PyHASH_MODULUS = (1 << 61) - 1
# Value to be used for rationals that reduce to infinity modulo
# _PyHASH_MODULUS.
_PyHASH_INF = 314159


@functools.lru_cache(maxsize=1 << 14)
def _hash_algorithm(numerator, denominator):

    # To make sure that the hash of a Fraction agrees with the hash
    # of a numerically equal integer or float instance, we follow the rules
    # for numeric hashes outlined in the documentation.
    try:
        dinv = pow(denominator, -1, _PyHASH_MODULUS)
    except ValueError:
        # ValueError means there is no modular inverse.
        hash_ = _PyHASH_INF
    else:
        hash_ = hash(hash(abs(numerator)) * dinv)
    result = hash_ if numerator >= 0 else -hash_
    return -2 if result == -1 else result


_RATIONAL_FORMAT = re.compile(r"""
    \A\s*                                  # optional whitespace at the start,
    (?P<sign>[-+]?)                        # an optional sign, then
    (?=\d|\.\d)                            # lookahead for digit or .digit
    (?P<num>\d*|\d+(_\d+)*)                # numerator (possibly empty)
    (?:                                    # followed by
       (?:\s*/\s*(?P<denom>\d+(_\d+)*))?   # an optional denominator
    |                                      # or
       (?:\.(?P<decimal>\d*|\d+(_\d+)*))?  # an optional fractional part
       (?:E(?P<exp>[-+]?\d+(_\d+)*))?      # and optional exponent
    )
    \s*\Z                                  # and optional whitespace to finish
""", re.VERBOSE | re.IGNORECASE)


# Helpers for formatting

def _round_to_exponent(n, d, exponent, no_neg_zero=False):
    """Round a rational number to the nearest multiple of a given power of 10."""
    if exponent >= 0:
        d *= 10**exponent
    else:
        n *= 10**-exponent

    # The divmod quotient is correct for round-ties-towards-positive-infinity;
    # In the case of a tie, we zero out the least significant bit of q.
    q, r = divmod(n + (d >> 1), d)
    if r == 0 and d & 1 == 0:
        q &= -2

    sign = q < 0 if no_neg_zero else n < 0
    return sign, abs(q)


def _round_to_figures(n, d, figures):
    """Round a rational number to a given number of significant figures."""
    # Special case for n == 0.
    if n == 0:
        return False, 0, 1 - figures

    # Find integer m satisfying 10**(m - 1) <= abs(n)/d <= 10**m.
    str_n, str_d = str(abs(n)), str(d)
    m = len(str_n) - len(str_d) + (str_d <= str_n)

    # Round to a multiple of 10**(m - figures).
    exponent = m - figures
    sign, significand = _round_to_exponent(n, d, exponent)

    # Adjust in the case where significand == 10**figures.
    if len(str(significand)) == figures + 1:
        significand //= 10
        exponent += 1

    return sign, significand, exponent


# Pattern for matching float-style format specifications.
_FLOAT_FORMAT_SPECIFICATION_MATCHER = re.compile(r"""
    (?:
        (?P<fill>.)?
        (?P<align>[<>=^])
    )?
    (?P<sign>[-+ ]?)
    (?P<no_neg_zero>z)?
    (?P<alt>\#)?
    # A '0' that's *not* followed by another digit is parsed as a minimum width
    # rather than a zeropad flag.
    (?P<zeropad>0(?=[0-9]))?
    (?P<minimumwidth>0|[1-9][0-9]*)?
    (?P<thousands_sep>[,_])?
    (?:\.(?P<precision>0|[1-9][0-9]*))?
    (?P<presentation_type>[eEfFgG%])
""", re.DOTALL | re.VERBOSE).fullmatch


def _numer(x):
    """numerator of an int or Fraction."""
    return x.numerator if isinstance(x, Fraction) else x


def _denom(x):
    """denominator of an int or Fraction."""
    return x.denominator if isinstance(x, Fraction) else 1


class Fraction:
    """This class implements rational numbers.

    In the two-argument form of the constructor, Fraction(8, 6) will
    produce a rational number equivalent to 4/3. Both arguments must
    be Rational. The numerator defaults to 0 and the denominator
    defaults to 1 so that Fraction(3) == 3 and Fraction() == 0.

    Fractions can also be constructed from:

      - numeric strings similar to those accepted by the
        float constructor (for example, '-2.3' or '1e10')

      - strings of the form '123/456'

      - float instances

      - other Fraction instances (and integers)

    """

    __slots__ = ('_numerator', '_denominator')

    # We're immutable, so use __new__ not __init__
    def __new__(cls, numerator=0, denominator=None):
        """Constructs a Rational.

        Takes a string like '3/2' or '1.5', another Fraction instance, a
        numerator/denominator pair, or a float.
        """
        self = super(Fraction, cls).__new__(cls)

        if denominator is None:
            if type(numerator) is int:
                self._numerator = numerator
                self._denominator = 1
                return self

            elif isinstance(numerator, Fraction):
                self._numerator = numerator.numerator
                self._denominator = numerator.denominator
                return self

            elif isinstance(numerator, float):
                # Exact conversion
                self._numerator, self._denominator = numerator.as_integer_ratio()
                return self

            elif isinstance(numerator, str):
                # Handle construction from strings.
                m = _RATIONAL_FORMAT.match(numerator)
                if m is None:
                    raise ValueError('Invalid literal for Fraction: %r' %
                                     numerator)
                numerator = int(m.group('num') or '0')
                denom = m.group('denom')
                if denom:
                    denominator = int(denom)
                else:
                    denominator = 1
                    decimal = m.group('decimal')
                    if decimal:
                        decimal = decimal.replace('_', '')
                        scale = 10**len(decimal)
                        numerator = numerator * scale + int(decimal)
                        denominator *= scale
                    exp = m.group('exp')
                    if exp:
                        exp = int(exp)
                        if exp >= 0:
                            numerator *= 10**exp
                        else:
                            denominator *= 10**-exp
                if m.group('sign') == '-':
                    numerator = -numerator

            else:
                raise TypeError("argument should be a string "
                                "or a Rational instance")

        elif type(numerator) is int is type(denominator):
            pass  # *very* normal case

        elif (isinstance(numerator, (int, Fraction)) and
              isinstance(denominator, (int, Fraction))):
            numerator, denominator = (
                _numer(numerator) * _denom(denominator),
                _numer(denominator) * _denom(numerator)
                )
        else:
            raise TypeError("both arguments should be "
                            "Rational instances")

        if denominator == 0:
            raise ZeroDivisionError('Fraction(%s, 0)' % numerator)
        g = math.gcd(numerator, denominator)
        if denominator < 0:
            g = -g
        numerator //= g
        denominator //= g
        self._numerator = numerator
        self._denominator = denominator
        return self

    @classmethod
    def from_float(cls, f):
        """Converts a finite float to a rational number, exactly.

        Beware that Fraction.from_float(0.3) != Fraction(3, 10).
        """
        if isinstance(f, int):
            return cls(f)
        elif not isinstance(f, float):
            raise TypeError("%s.from_float() only takes floats, not %r (%s)" %
                            (cls.__name__, f, type(f).__name__))
        return cls._from_coprime_ints(*f.as_integer_ratio())

    @classmethod
    def _from_coprime_ints(cls, numerator, denominator):
        """Convert a pair of ints to a rational number, for internal use.

        The ratio of integers should be in lowest terms and the denominator
        should be positive.
        """
        obj = super(Fraction, cls).__new__(cls)
        obj._numerator = numerator
        obj._denominator = denominator
        return obj

    def is_integer(self):
        """Return True if the Fraction is an integer."""
        return self._denominator == 1

    def as_integer_ratio(self):
        """Return a pair of integers, whose ratio is equal to the original Fraction.

        The ratio is in lowest terms and has a positive denominator.
        """
        return (self._numerator, self._denominator)

    def limit_denominator(self, max_denominator=1000000):
        """Closest Fraction to self with denominator at most max_denominator.

        >>> Fraction('3.141592653589793').limit_denominator(10)
        Fraction(22, 7)
        >>> Fraction('3.141592653589793').limit_denominator(100)
        Fraction(311, 99)
        >>> Fraction(4321, 8765).limit_denominator(10000)
        Fraction(4321, 8765)
        """
        if max_denominator < 1:
            raise ValueError("max_denominator should be at least 1")
        if self._denominator <= max_denominator:
            return Fraction(self)

        p0, q0, p1, q1 = 0, 1, 1, 0
        n, d = self._numerator, self._denominator
        while True:
            a = n//d
            q2 = q0+a*q1
            if q2 > max_denominator:
                break
            p0, q0, p1, q1 = p1, q1, p0+a*p1, q2
            n, d = d, n-a*d
        k = (max_denominator-q0)//q1

        # Determine which of the candidates is closer to self.
        if 2*d*(q0+k*q1) <= self._denominator:
            return Fraction._from_coprime_ints(p1, q1)
        else:
            return Fraction._from_coprime_ints(p0+k*p1, q0+k*q1)

    @property
    def numerator(a):
        return a._numerator

    @property
    def denominator(a):
        return a._denominator

    def __repr__(self):
        """repr(self)"""
        return '%s(%s, %s)' % (self.__class__.__name__,
                               self._numerator, self._denominator)

    def __str__(self):
        """str(self)"""
        if self._denominator == 1:
            return str(self._numerator)
        else:
            return '%s/%s' % (self._numerator, self._denominator)

    def __format__(self, format_spec, /):
        """Format this fraction according to the given format specification."""

        # Backwards compatiblility with existing formatting.
        if not format_spec:
            return str(self)

        # Validate and parse the format specifier.
        match = _FLOAT_FORMAT_SPECIFICATION_MATCHER(format_spec)
        if match is None:
            raise ValueError(
                f"Invalid format specifier {format_spec!r} "
                f"for object of type {type(self).__name__!r}"
            )
        elif match["align"] is not None and match["zeropad"] is not None:
            # Avoid the temptation to guess.
            raise ValueError(
                f"Invalid format specifier {format_spec!r} "
                f"for object of type {type(self).__name__!r}; "
                "can't use explicit alignment when zero-padding"
            )
        fill = match["fill"] or " "
        align = match["align"] or ">"
        pos_sign = "" if match["sign"] == "-" else match["sign"]
        no_neg_zero = bool(match["no_neg_zero"])
        alternate_form = bool(match["alt"])
        zeropad = bool(match["zeropad"])
        minimumwidth = int(match["minimumwidth"] or "0")
        thousands_sep = match["thousands_sep"]
        precision = int(match["precision"] or "6")
        presentation_type = match["presentation_type"]
        trim_zeros = presentation_type in "gG" and not alternate_form
        trim_point = not alternate_form
        exponent_indicator = "E" if presentation_type in "EFG" else "e"

        # Round to get the digits we need, figure out where to place the point,
        # and decide whether to use scientific notation.
        if presentation_type in "fF%":
            exponent = -precision
            if presentation_type == "%":
                exponent -= 2
            negative, significand = _round_to_exponent(
                self._numerator, self._denominator, exponent, no_neg_zero)
            scientific = False
            point_pos = precision
        else:  # presentation_type in "eEgG"
            figures = (
                max(precision, 1)
                if presentation_type in "gG"
                else precision + 1
            )
            negative, significand, exponent = _round_to_figures(
                self._numerator, self._denominator, figures)
            scientific = (
                presentation_type in "eE"
                or exponent > 0
                or exponent + figures <= -4
            )
            point_pos = figures - 1 if scientific else -exponent

        # Get the suffix - the part following the digits, if any.
        if presentation_type == "%":
            suffix = "%"
        elif scientific:
            suffix = f"{exponent_indicator}{exponent + point_pos:+03d}"
        else:
            suffix = ""

        # String of output digits, padded sufficiently with zeros on the left.
        digits = f"{significand:0{point_pos + 1}d}"

        # Before padding, the output has the form f"{sign}{leading}{trailing}".
        sign = "-" if negative else pos_sign
        leading = digits[: len(digits) - point_pos]
        frac_part = digits[len(digits) - point_pos :]
        if trim_zeros:
            frac_part = frac_part.rstrip("0")
        separator = "" if trim_point and not frac_part else "."
        trailing = separator + frac_part + suffix

        # Do zero padding if required.
        if zeropad:
            min_leading = minimumwidth - len(sign) - len(trailing)
            leading = leading.zfill(
                3 * min_leading // 4 + 1 if thousands_sep else min_leading
            )

        # Insert thousands separators if required.
        if thousands_sep:
            first_pos = 1 + (len(leading) - 1) % 3
            leading = leading[:first_pos] + "".join(
                thousands_sep + leading[pos : pos + 3]
                for pos in range(first_pos, len(leading), 3)
            )

        # We now have a sign and a body. Pad with fill character if necessary.
        body = leading + trailing
        padding = fill * (minimumwidth - len(sign) - len(body))
        if align == ">":
            return padding + sign + body
        elif align == "<":
            return sign + body + padding
        elif align == "^":
            half = len(padding) // 2
            return padding[:half] + sign + body + padding[half:]
        else:  # align == "="
            return sign + padding + body

    def _operator_fallbacks(monomorphic_operator, fallback_operator):
        """Generates forward and reverse operators given a purely-rational
        operator and a function from the operator module.
        """
        def forward(a, b):
            if isinstance(b, Fraction):
                return monomorphic_operator(a, b)
            elif isinstance(b, int):
                return monomorphic_operator(a, Fraction(b))
            elif isinstance(b, float):
                return fallback_operator(float(a), b)
            elif isinstance(b, complex):
                return fallback_operator(complex(a), b)
            else:
                return NotImplemented
        forward.__name__ = '__' + fallback_operator.__name__ + '__'
        forward.__doc__ = monomorphic_operator.__doc__

        def reverse(b, a):
            if isinstance(a, (int, Fraction)):
                # Includes ints.
                return monomorphic_operator(Fraction(a), b)
            elif isinstance(a, float):
                return fallback_operator(float(a), float(b))
            elif isinstance(a, complex):
                return fallback_operator(complex(a), complex(b))
            else:
                return NotImplemented
        reverse.__name__ = '__r' + fallback_operator.__name__ + '__'
        reverse.__doc__ = monomorphic_operator.__doc__

        return forward, reverse

    # Rational arithmetic algorithms: Knuth, TAOCP, Volume 2, 4.5.1.

    def _add(a, b):
        """a + b"""
        na, da = a._numerator, a._denominator
        nb, db = b._numerator, b._denominator
        g = math.gcd(da, db)
        if g == 1:
            return Fraction._from_coprime_ints(na * db + da * nb, da * db)
        s = da // g
        t = na * (db // g) + nb * s
        g2 = math.gcd(t, g)
        if g2 == 1:
            return Fraction._from_coprime_ints(t, s * db)
        return Fraction._from_coprime_ints(t // g2, s * (db // g2))

    __add__, __radd__ = _operator_fallbacks(_add, operator.add)

    def _sub(a, b):
        """a - b"""
        na, da = a._numerator, a._denominator
        nb, db = b._numerator, b._denominator
        g = math.gcd(da, db)
        if g == 1:
            return Fraction._from_coprime_ints(na * db - da * nb, da * db)
        s = da // g
        t = na * (db // g) - nb * s
        g2 = math.gcd(t, g)
        if g2 == 1:
            return Fraction._from_coprime_ints(t, s * db)
        return Fraction._from_coprime_ints(t // g2, s * (db // g2))

    __sub__, __rsub__ = _operator_fallbacks(_sub, operator.sub)

    def _mul(a, b):
        """a * b"""
        na, da = a._numerator, a._denominator
        nb, db = b._numerator, b._denominator
        g1 = math.gcd(na, db)
        if g1 > 1:
            na //= g1
            db //= g1
        g2 = math.gcd(nb, da)
        if g2 > 1:
            nb //= g2
            da //= g2
        return Fraction._from_coprime_ints(na * nb, db * da)

    __mul__, __rmul__ = _operator_fallbacks(_mul, operator.mul)

    def _div(a, b):
        """a / b"""
        # Same as _mul(), with inversed b.
        nb, db = b._numerator, b._denominator
        if nb == 0:
            raise ZeroDivisionError('Fraction(%s, 0)' % db)
        na, da = a._numerator, a._denominator
        g1 = math.gcd(na, nb)
        if g1 > 1:
            na //= g1
            nb //= g1
        g2 = math.gcd(db, da)
        if g2 > 1:
            da //= g2
            db //= g2
        n, d = na * db, nb * da
        if d < 0:
            n, d = -n, -d
        return Fraction._from_coprime_ints(n, d)

    __truediv__, __rtruediv__ = _operator_fallbacks(_div, operator.truediv)

    def _floordiv(a, b):
        """a // b"""
        return (a.numerator * b.denominator) // (a.denominator * b.numerator)

    __floordiv__, __rfloordiv__ = _operator_fallbacks(_floordiv, operator.floordiv)

    def _divmod(a, b):
        """(a // b, a % b)"""
        da, db = a.denominator, b.denominator
        div, n_mod = divmod(a.numerator * db, da * b.numerator)
        return div, Fraction(n_mod, da * db)

    __divmod__, __rdivmod__ = _operator_fallbacks(_divmod, divmod)

    def _mod(a, b):
        """a % b"""
        da, db = a.denominator, b.denominator
        return Fraction((a.numerator * db) % (b.numerator * da), da * db)

    __mod__, __rmod__ = _operator_fallbacks(_mod, operator.mod)

    def __pow__(a, b):
        """a ** b

        If b is not an integer, the result will be a float or complex
        since roots are generally irrational. If b is an integer, the
        result will be rational.
        """
        if isinstance(b, (int, Fraction)):
            if b.denominator == 1:
                power = b.numerator
                if power >= 0:
                    return Fraction._from_coprime_ints(a._numerator ** power,
                                                       a._denominator ** power)
                elif a._numerator > 0:
                    return Fraction._from_coprime_ints(a._denominator ** -power,
                                                       a._numerator ** -power)
                elif a._numerator == 0:
                    raise ZeroDivisionError('Fraction(%s, 0)' %
                                            a._denominator ** -power)
                else:
                    return Fraction._from_coprime_ints((-a._denominator) ** -power,
                                                       (-a._numerator) ** -power)
            else:
                # A fractional power will generally produce an
                # irrational number.
                return float(a) ** float(b)
        else:
            return float(a) ** b

    def __rpow__(b, a):
        """a ** b"""
        if b._denominator == 1 and b._numerator >= 0:
            # If a is an int, keep it that way if possible.
            return a ** b._numerator

        if isinstance(a, (int, Fraction)):
            return Fraction(a.numerator, a.denominator) ** b

        if b._denominator == 1:
            return a ** b._numerator

        return a ** float(b)

    def __pos__(a):
        """+a: Coerces a subclass instance to Fraction"""
        return Fraction._from_coprime_ints(a._numerator, a._denominator)

    def __neg__(a):
        """-a"""
        return Fraction._from_coprime_ints(-a._numerator, a._denominator)

    def __abs__(a):
        """abs(a)"""
        return Fraction._from_coprime_ints(abs(a._numerator), a._denominator)

    def __int__(a, _index=operator.index):
        """int(a)"""
        if a._numerator < 0:
            return _index(-(-a._numerator // a._denominator))
        else:
            return _index(a._numerator // a._denominator)

    def __float__(a):
        """float(self) = self.numerator / self.denominator

        Provided directly (CPython inherits it from numbers.Rational, which
        pyrust does not ship).  Uses integer true division so ratios of huge
        integers convert without overflowing.
        """
        return a._numerator / a._denominator

    def __complex__(a):
        """complex(self)"""
        return complex(float(a))

    def __trunc__(a):
        """math.trunc(a)"""
        if a._numerator < 0:
            return -(-a._numerator // a._denominator)
        else:
            return a._numerator // a._denominator

    def __floor__(a):
        """math.floor(a)"""
        return a._numerator // a._denominator

    def __ceil__(a):
        """math.ceil(a)"""
        # The negations cleverly convince floordiv to return the ceiling.
        return -(-a._numerator // a._denominator)

    def __round__(self, ndigits=None):
        """round(self, ndigits)

        Rounds half toward even.
        """
        if ndigits is None:
            d = self._denominator
            floor, remainder = divmod(self._numerator, d)
            if remainder * 2 < d:
                return floor
            elif remainder * 2 > d:
                return floor + 1
            # Deal with the half case:
            elif floor % 2 == 0:
                return floor
            else:
                return floor + 1
        shift = 10**abs(ndigits)
        if ndigits > 0:
            return Fraction(round(self * shift), shift)
        else:
            return Fraction(round(self / shift) * shift)

    def __hash__(self):
        """hash(self)"""
        return _hash_algorithm(self._numerator, self._denominator)

    def __eq__(a, b):
        """a == b"""
        if type(b) is int:
            return a._numerator == b and a._denominator == 1
        # CPython uses ``isinstance(b, numbers.Rational)`` here, which covers
        # ``int`` and ``bool`` (both registered as ``Rational``) as well as
        # ``Fraction``.  pyrust ships no ``numbers`` ABC, so spell out the
        # concrete rational types -- ``int`` here includes ``bool``, and an
        # ``int``/``bool`` exposes ``.numerator`` / ``.denominator``.
        if isinstance(b, (int, Fraction)):
            return (a._numerator == b.numerator and
                    a._denominator == b.denominator)
        if isinstance(b, complex) and b.imag == 0:
            b = b.real
        if isinstance(b, float):
            if math.isnan(b) or math.isinf(b):
                # comparisons with an infinity or nan should behave in
                # the same way for any finite a, so treat a as zero.
                return 0.0 == b
            else:
                return a == a.from_float(b)
        else:
            # Since a doesn't know how to compare with b, let's give b
            # a chance to compare itself with a.
            return NotImplemented

    def _richcmp(self, other, op):
        """Helper for comparison operators, for internal use only."""
        # convert other to a Fraction instance where reasonable.
        if isinstance(other, (int, Fraction)):
            return op(self._numerator * other.denominator,
                      self._denominator * other.numerator)
        if isinstance(other, float):
            if math.isnan(other) or math.isinf(other):
                return op(0.0, other)
            else:
                return op(self, self.from_float(other))
        else:
            return NotImplemented

    def __lt__(a, b):
        """a < b"""
        return a._richcmp(b, operator.lt)

    def __gt__(a, b):
        """a > b"""
        return a._richcmp(b, operator.gt)

    def __le__(a, b):
        """a <= b"""
        return a._richcmp(b, operator.le)

    def __ge__(a, b):
        """a >= b"""
        return a._richcmp(b, operator.ge)

    def __bool__(a):
        """a != 0"""
        return bool(a._numerator)

    # support for pickling, copy, and deepcopy

    def __reduce__(self):
        return (self.__class__, (self._numerator, self._denominator))

    def __copy__(self):
        if type(self) == Fraction:
            return self     # I'm immutable; therefore I am my own clone
        return self.__class__(self._numerator, self._denominator)

    def __deepcopy__(self, memo):
        if type(self) == Fraction:
            return self     # My components are also immutable
        return self.__class__(self._numerator, self._denominator)
