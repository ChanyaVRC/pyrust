# Issue #3412: `fractions` is implemented natively in Rust.  This fixture pins
# the CPython 3.12.3 behaviour of the whole `Fraction` surface: construction
# from every accepted type, the `_RATIONAL_FORMAT` string grammar, exact
# arithmetic and its float / complex / Decimal fallbacks, comparisons, the
# numeric hash, rounding, `limit_denominator`, the 3.12 float-style
# `__format__` mini-language, argument binding errors, pickling / copying and
# subclass behaviour.
#
# Not pinned (deliberate or runtime-wide differences): the module's `Decimal`,
# `functools`, `re`, `_RATIONAL_FORMAT` and `_FLOAT_FORMAT_SPECIFICATION_MATCHER`
# globals and the class-body `_operator_fallbacks` helper (omitted so that
# `import fractions` does not import the pure-Python `decimal` / `re`
# modules); method `__doc__` / `__qualname__` and `type()` of slot descriptors
# (pyrust-wide native-method gaps).

import copy
import math
import numbers
import operator
import pickle
import sys
from decimal import Decimal

import fractions
from fractions import Fraction as F


def show(label, thunk):
    try:
        result = thunk()
    except Exception as exc:  # noqa: BLE001 - the error is the observable
        print(label, "!!", type(exc).__name__, exc)
    else:
        print(label, "->", repr(result))


# ── Module and class surface ────────────────────────────────────────────────
print(fractions.__all__, fractions.__doc__)
print(fractions._PyHASH_MODULUS == sys.hash_info.modulus, fractions._PyHASH_INF == sys.hash_info.inf)
print(fractions.math is math, fractions.numbers is numbers, fractions.operator is operator, fractions.sys is sys)
print(F.__module__, F.__name__, F.__qualname__, type(F).__name__)
print(F.__mro__)
print(F.__abstractmethods__, F.__slots__, F.__doc__.splitlines()[0])
print([name for name in vars(F) if name != "_operator_fallbacks"])
print(issubclass(F, numbers.Rational), isinstance(F(1), numbers.Number), isinstance(F(1), numbers.Integral))
print(hasattr(F(1), "__dict__"), "__weakref__" in dir(F))
show("setattr", lambda: setattr(F(1), "x", 1))

# ── Construction ────────────────────────────────────────────────────────────
for args in [(), (7,), (-7,), (6, 4), (6, -4), (-6, -4), (0, 5), (10**30, 10**20),
             (True,), (True, 2), (3, True), (F(1, 3),), (F(1, 3), F(2, 5)),
             (F(5, 7), 3), (2, F(3, 4))]:
    show(f"F{args!r}", lambda args=args: F(*args))
show("kw", lambda: F(numerator=3, denominator=-6))
show("kw-den", lambda: F(denominator=2))
show("None-den", lambda: F(3, None))

for value in [0.0, -0.0, 0.5, -2.25, 0.1, 1e300, 5e-324, 2.0**60, float("nan"),
              float("inf"), -float("inf")]:
    show(f"F({value!r})", lambda value=value: F(value))

for text in ["3/4", " -35/4 ", "\t+7\n", "3 / 4", "1_000/3_0", "1.5", "-.5",
             "1.", "1.e3", "1e-3", "1E+2", "-47e-2", "1_000.000_1", "2.5e1_0",
             "0/1", "00012", "١٢/٣", " 3/4 ",
             "", " ", ".", "1_", "_1", "1__0", "1/", "/2", "1e", "1.d", "1/2.5",
             "3/4/5", "1 .5", "+-1", "0x10", "inf", "nan", "1/0", "3/-4", "1,5",
             "٣.٤e١"]:
    show(f"F({text!r})", lambda text=text: F(text))


class Str(str):
    pass


show("str subclass", lambda: F(Str("5/10")))
show("bad str subclass", lambda: F(Str("five")))

for bad in [None, [1], 1j, b"1/2", object]:
    show(f"F({bad!r})", lambda bad=bad: F(bad))
show("F(1.5, 2)", lambda: F(1.5, 2))
show("F(1, 2.0)", lambda: F(1, 2.0))
show("F('1', 2)", lambda: F("1", 2))
show("F(3, 0)", lambda: F(3, 0))
show("F(0, 0)", lambda: F(0, 0))
show("F(F(1, 2), F(0))", lambda: F(F(1, 2), F(0)))

show("Decimal", lambda: F(Decimal("-1.25")))
show("Decimal exp", lambda: F(Decimal("3E+2")))
show("Decimal nan", lambda: F(Decimal("NaN")))
show("Decimal inf", lambda: F(Decimal("-Infinity")))


class MyInt(int):
    pass


class MyFloat(float):
    pass


class MyRational(numbers.Rational):
    def __init__(self, n, d):
        self.n, self.d = n, d

    numerator = property(lambda self: self.n)
    denominator = property(lambda self: self.d)

    def __repr__(self):
        return f"MyRational({self.n}, {self.d})"

    __abs__ = __add__ = __ceil__ = __eq__ = __floor__ = __floordiv__ = None
    __le__ = __lt__ = __mod__ = __mul__ = __neg__ = __pos__ = __pow__ = None
    __radd__ = __rfloordiv__ = __rmod__ = __rmul__ = __round__ = None
    __rpow__ = __rtruediv__ = __truediv__ = __trunc__ = None


show("MyInt", lambda: F(MyInt(12), MyInt(-8)))
show("MyInt alone", lambda: F(MyInt(12)))
show("MyFloat", lambda: F(MyFloat(0.75)))
show("MyRational", lambda: F(MyRational(2, 4)))
show("MyRational pair", lambda: F(MyRational(2, 4), MyRational(3, 9)))
show("MyRational + F", lambda: F(1, 3) + MyRational(1, 6))

# ── Alternate constructors and accessors ────────────────────────────────────
for value in [0.25, -3.0, 7, True, MyFloat(1.5), F(1, 2), "x", float("nan"), float("inf")]:
    show(f"from_float({value!r})", lambda value=value: F.from_float(value))
for value in [Decimal("1.1"), Decimal("-0"), 5, True, 1.5, Decimal("NaN")]:
    show(f"from_decimal({value!r})", lambda value=value: F.from_decimal(value))
show("_from_coprime_ints", lambda: F._from_coprime_ints(2, 4))
show("_from_coprime_ints + 1", lambda: F._from_coprime_ints(2, 4) + 1)

f = F(-22, 7)
print(f.numerator, f.denominator, f._numerator, f._denominator, f.as_integer_ratio())
print(f.is_integer(), F(4, 2).is_integer(), f.real, f.imag, f.conjugate())
print(type(f.numerator).__name__, type(f.real).__name__)

# ── Arithmetic ───────────────────────────────────────────────────────────────
OPS = [
    ("+", operator.add), ("-", operator.sub), ("*", operator.mul),
    ("/", operator.truediv), ("//", operator.floordiv), ("%", operator.mod),
    ("divmod", divmod),
]
LEFT = [F(1, 3), F(-5, 7), F(0), F(10**20 + 1, 7)]
RIGHT = [F(2, 9), F(-1, 4), F(0), 3, -2, 0, True, MyInt(4), 0.5, -1.25, 0.0,
         2 + 3j, Decimal("1.5"), "x", None]
for left in LEFT:
    for right in RIGHT:
        for symbol, op in OPS:
            show(f"{left!r} {symbol} {right!r}", lambda: op(left, right))
            show(f"{right!r} {symbol} {left!r}", lambda: op(right, left))

for base in [F(2, 3), F(-2, 3), F(0), F(9, 4), F(-8, 27)]:
    for exponent in [0, 1, 3, -2, F(1, 2), F(-3), F(2, 1), 0.5, -1.0, 1j, MyInt(2), True, "x"]:
        show(f"{base!r} ** {exponent!r}", lambda: base ** exponent)
for base in [2, -8, 0, 2.0, -2.0, 1j, MyInt(4), Decimal("4"), "x"]:
    for exponent in [F(3), F(-2), F(1, 2), F(2, 3), F(0)]:
        show(f"{base!r} ** {exponent!r}", lambda: base ** exponent)
show("pow 3-arg", lambda: pow(F(1, 2), 2, 3))

for value in [F(7, 2), F(-7, 2), F(5, 2), F(-5, 2), F(1, 3), F(0), F(10**25, 3)]:
    print(repr(value), +value, -value, abs(value), int(value), math.trunc(value),
          math.floor(value), math.ceil(value), round(value), bool(value),
          float(value), complex(value))
print(F(7, 2).__int__(lambda x: ("index", x)))
for value in [F(12345, 100), F(-12355, 1000), F(1, 3), F(25, 10)]:
    for ndigits in [None, 0, 1, 2, -1, -2, True, False]:
        show(f"round({value!r}, {ndigits!r})", lambda: round(value, ndigits))
show("round float ndigits", lambda: round(F(1, 3), 2.0))
show("round str ndigits", lambda: round(F(1, 3), "2"))


class Sub(F):
    pass


s = Sub(3, 6)
print(repr(s), str(s), type(s + 1).__name__, type(1 + s).__name__, type(-s).__name__,
      type(s * s).__name__, type(round(s, 1)).__name__)
print(hasattr(s, "__dict__"), isinstance(s, F), type(Sub.from_float(0.5)).__name__,
      repr(Sub.from_decimal(Decimal("0.5"))), repr(Sub._from_coprime_ints(1, 3)))

# ── Comparisons and hashing ──────────────────────────────────────────────────
CMP_LEFT = [F(1, 2), F(-3), F(0)]
CMP_RIGHT = [F(1, 2), F(2, 4), 0, -3, True, False, 0.5, -3.0, float("nan"), float("inf"),
             -float("inf"), 0.5 + 0j, 0.5 + 1j, Decimal("0.5"), Decimal("-3"), MyInt(-3),
             MyFloat(0.5), "x", None]
for left in CMP_LEFT:
    for right in CMP_RIGHT:
        for symbol in ["==", "!=", "<", "<=", ">", ">="]:
            expr = f"left {symbol} right"
            show(f"{left!r} {symbol} {right!r}", lambda: eval(expr))
            show(f"{right!r} {symbol} {left!r}", lambda: eval(f"right {symbol} left"))
print(sorted([F(1, 2), 0.25, 1, F(-1, 3), Decimal("0.3"), True]))

for value in [F(0), F(1), F(-1), F(1, 2), F(-1, 2), F(5, 3), F(10**30, 7), F(2**61 - 2, 3),
              F(1, 2**61 - 1), F(-1, 2**61 - 1), F(2**61 - 1, 2**62), F(2**80)]:
    print(repr(value), hash(value))
for value in [F(3), F(-7, 4), F(1, 1024), F(10**20)]:
    print(hash(value) == hash(float(value)), hash(value) == hash(Decimal(value.numerator) / value.denominator))
print(hash(F(5)) == hash(5), {F(1, 2): "half"}[0.5], {0.5: "half"}[F(1, 2)], len({F(2, 4), F(1, 2)}))
print(fractions._hash_algorithm(1, 2), fractions._hash_algorithm(-3, 2**61 - 1))
print(fractions._round_to_exponent(1, 3, -2), fractions._round_to_exponent(-1, 3, 0, True))
print(fractions._round_to_exponent(5, 2, 0), fractions._round_to_exponent(7, 2, 0))
print(fractions._round_to_figures(1, 3, 3), fractions._round_to_figures(0, 1, 2),
      fractions._round_to_figures(-999, 1, 2))

# ── limit_denominator ───────────────────────────────────────────────────────
pi = F("3.141592653589793")
for limit in [1, 2, 7, 10, 100, 1000, 10**6, 10**20, True]:
    show(f"pi.limit_denominator({limit!r})", lambda: pi.limit_denominator(limit))
for value in [F(4321, 8765), F(-7, 5), F(1, 2), F(3, 2), F(-3, 2), F(10**20 + 1, 10**20)]:
    for limit in [1, 3, 100]:
        show(f"{value!r}.limit_denominator({limit})", lambda: value.limit_denominator(limit))
show("limit 0", lambda: F(1, 3).limit_denominator(0))
show("limit -5", lambda: F(1, 3).limit_denominator(-5))
show("limit float fits", lambda: F(1, 3).limit_denominator(10.0))
show("limit float small", lambda: F(1, 3).limit_denominator(0.5))
print(F(7).limit_denominator() is F(7), F(1, 3).limit_denominator().__class__.__name__,
      type(s.limit_denominator()).__name__)

# ── __format__ ───────────────────────────────────────────────────────────────
VALUES = [F(0), F(-0), F(1, 3), F(-2, 3), F(22, 7), F(-1, 200), F(123456789, 1000),
          F(10**12, 7), F(5, 2), F(-5, 2), F(1, 10**8), F(-1, 10**20)]
SPECS = ["", "f", ".0f", ".2f", "F", "e", ".3e", "E", ".0e", "g", ".1g", ".3g", "G", "#g",
         "#.0f", "#.0e", "%", ".1%", "+f", " f", "-f", "z.1f", "z.0e", "10.2f", "<10.2f",
         "*^12.3f", "=+12.2f", "010.2f", "0=10.2f", "08.3e", ",.2f", "_.3f", "020,.2f",
         "015_.1f", "^,.2f", "+#012,.4g", " >9.1f", "\n<6.1f", ".30f", ".20e", ".25g"]
for value in VALUES:
    print(repr(value), [format(value, spec) for spec in SPECS])
print(f"{F(355, 113):>12.6f}|{F(-1, 7):+.3%}|{F(1, 3)!s:>6}|{F(2):.0f}")
for spec in ["d", "x", "n", "10", "<", "=", ".2", "05", "005f", ".01f", "_,f", "f%",
             "<<<f", "éf", "0<5f", "<05f", "z", "#", "q.2f"]:
    show(f"format(F(1, 3), {spec!r})", lambda: format(F(1, 3), spec))
show("format digits limit", lambda: format(F(1, 3), ".5000f"))

# ── str / repr and the int digit limit ──────────────────────────────────────
print(str(F(-6, 4)), repr(F(-6, 4)), str(F(8, 4)), repr(F(8)), f"{F(1, 3)}")
big = F(10**4500, 3)
show("str big", lambda: str(big))
show("repr big", lambda: repr(big))
show("parse big", lambda: F("1" * 4301))
show("parse big denom", lambda: F("1/" + "1" * 4301))
show("parse big decimal", lambda: F("0." + "1" * 4301))
show("parse big exp", lambda: F("1e" + "1" * 4301))
print(len(str(F(10**4200, 3))))

# ── Argument binding errors ─────────────────────────────────────────────────
show("F(1, 2, 3)", lambda: F(1, 2, 3))
show("F(foo=1)", lambda: F(foo=1))
show("F(1, numerator=2)", lambda: F(1, numerator=2))
show("__new__()", lambda: F.__new__())
show("__new__(int)", lambda: F.__new__(int, 1))
show("__new__(5)", lambda: F.__new__(5))
show("_from_coprime_ints(1)", lambda: F._from_coprime_ints(1))
show("_from_coprime_ints kw", lambda: F._from_coprime_ints(numerator=1, denominator=2))
show("limit_denominator()", lambda: F.limit_denominator())
show("limit_denominator(1, 2)", lambda: F(1).limit_denominator(1, 2))
show("limit_denominator kw", lambda: F(1, 3).limit_denominator(max_denominator=2))
show("limit_denominator bad kw", lambda: F(1, 3).limit_denominator(max_d=2))
show("from_float()", lambda: F.from_float())
show("__add__()", lambda: F.__add__(F(1)))
show("__radd__ 3", lambda: F.__radd__(F(1), 2, 3))
show("__round__ kw", lambda: F(7, 2).__round__(ndigits=1))
show("__deepcopy__()", lambda: F(1).__deepcopy__())
show("is_integer(1)", lambda: F(1).is_integer(1))
show("__format__()", lambda: F.__format__(F(1)))
show("__format__ kw", lambda: F.__format__(F(1), format_spec="f"))
show("__int__ kw", lambda: F(7, 2).__int__(_index=lambda x: -x))
show("_richcmp", lambda: F(1, 2)._richcmp(1, operator.lt))
show("_richcmp float", lambda: F(1, 2)._richcmp(0.25, operator.gt))
show("_richcmp nan", lambda: F(1, 2)._richcmp(float("nan"), operator.eq))
show("_add", lambda: F._add(F(1, 2), F(1, 3)))
show("_divmod", lambda: F._divmod(F(7, 2), F(1, 3)))
show("non-fraction self", lambda: F.__add__(5, F(1)))
show("non-fraction self float", lambda: F.__add__(5, 1.5))
show("non-fraction repr", lambda: F.__repr__(5))

# ── Pickling and copying ────────────────────────────────────────────────────
f = F(-3, 8)
print(f.__reduce__(), s.__reduce__())
print(copy.copy(f) is f, copy.deepcopy(f) is f, copy.copy(s) is s, repr(copy.deepcopy(s)))
for protocol in range(pickle.HIGHEST_PROTOCOL + 1):
    round_trip = pickle.loads(pickle.dumps([f, s, F(10**30, 7)], protocol))
    print(protocol, round_trip, [type(x).__name__ for x in round_trip])
f._numerator = 5
print(repr(f))

print("fractions native ok")
