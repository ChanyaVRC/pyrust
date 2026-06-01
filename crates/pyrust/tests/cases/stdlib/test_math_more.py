# Parity tests for math module functions added in issue #1885.
# Covers: degrees, radians, sinh, cosh, tanh, asinh, acosh, atanh, expm1,
# log1p, exp2, cbrt, fmod, remainder, modf, frexp, ldexp, nextafter, ulp,
# isqrt, isclose — plus their domain-error / edge-case behaviour.
import math

# ── degrees / radians ───────────────────────────────────────────────────────
print(math.degrees(math.pi))        # 180.0
print(math.degrees(0))              # 0.0
print(math.degrees(-math.pi / 2))   # -90.0
print(math.radians(180))            # 3.141592653589793
print(math.radians(90))             # 1.5707963267948966
print(math.radians(0))              # 0.0

# ── hyperbolic ──────────────────────────────────────────────────────────────
print(math.sinh(0))                 # 0.0
print(math.sinh(1))
print(math.cosh(0))                 # 1.0
print(math.cosh(1))
print(math.tanh(0))                 # 0.0
print(math.tanh(1))
print(math.asinh(0))                # 0.0
print(math.asinh(1))
print(math.acosh(1))                # 0.0
print(math.acosh(2))
print(math.atanh(0))                # 0.0
print(math.atanh(0.5))

try:
    math.acosh(0.5)
except ValueError as e:
    print("acosh(0.5): ValueError:", e)
try:
    math.atanh(1)
except ValueError as e:
    print("atanh(1): ValueError:", e)
try:
    math.atanh(-1)
except ValueError as e:
    print("atanh(-1): ValueError:", e)
try:
    math.atanh(2)
except ValueError as e:
    print("atanh(2): ValueError:", e)

# Overflow direction: sinh/cosh/expm1/exp2 overflowing (incl. to -inf for the
# odd functions) is a *range* error (OverflowError), not a domain error.
try:
    math.sinh(1000)
except OverflowError as e:
    print("sinh(1000): OverflowError:", e)
try:
    math.sinh(-1000)
except OverflowError as e:
    print("sinh(-1000): OverflowError:", e)
try:
    math.cosh(1000)
except OverflowError as e:
    print("cosh(1000): OverflowError:", e)
try:
    math.expm1(1000)
except OverflowError as e:
    print("expm1(1000): OverflowError:", e)

# ── expm1 / log1p / exp2 ────────────────────────────────────────────────────
print(math.expm1(0))                # 0.0
print(math.expm1(1))
print(math.log1p(0))                # 0.0
print(math.log1p(1))
print(math.exp2(0))                 # 1.0
print(math.exp2(3))                 # 8.0
print(math.exp2(0.5))

try:
    math.log1p(-1)
except ValueError as e:
    print("log1p(-1): ValueError:", e)
try:
    math.exp2(2000)
except OverflowError as e:
    print("exp2(2000): OverflowError:", e)

# ── cbrt (only inputs that agree bit-for-bit with CPython 3.12) ──────────────
print(math.cbrt(0.0))               # 0.0
print(math.cbrt(-0.0))              # -0.0
print(math.cbrt(8.0))               # 2.0
print(math.cbrt(-8.0))              # -2.0
print(math.cbrt(float('inf')))      # inf
print(math.cbrt(float('-inf')))     # -inf

# ── fmod (C semantics: sign of x) ───────────────────────────────────────────
print(math.fmod(7, 3))              # 1.0
print(math.fmod(-5, 3))             # -2.0
print(math.fmod(5, -3))             # 2.0
print(math.fmod(-5, -3))            # -2.0
print(math.fmod(7.5, 2))            # 1.5
print(math.fmod(1, math.inf))       # 1.0
print(math.fmod(math.nan, 1))       # nan

try:
    math.fmod(1, 0)
except ValueError as e:
    print("fmod(1, 0): ValueError:", e)
try:
    math.fmod(math.inf, 1)
except ValueError as e:
    print("fmod(inf, 1): ValueError:", e)

# ── remainder (IEEE 754, ties to even) ──────────────────────────────────────
print(math.remainder(7, 3))         # 1.0
print(math.remainder(-7, 3))        # -1.0
print(math.remainder(5, 2))         # 1.0
print(math.remainder(2.5, 2))       # 0.5
print(math.remainder(3.5, 2))       # -0.5
print(math.remainder(1, math.inf))  # 1.0

try:
    math.remainder(math.inf, 1)
except ValueError as e:
    print("remainder(inf, 1): ValueError:", e)
try:
    math.remainder(1, 0)
except ValueError as e:
    print("remainder(1, 0): ValueError:", e)

# ── modf ────────────────────────────────────────────────────────────────────
print(math.modf(3.5))               # (0.5, 3.0)
print(math.modf(-3.5))              # (-0.5, -3.0)
print(math.modf(0.0))               # (0.0, 0.0)
print(math.modf(-0.0))              # (-0.0, -0.0)
print(math.modf(5.0))               # (0.0, 5.0)
print(math.modf(-5.0))              # (-0.0, -5.0)
print(math.modf(math.inf))          # (0.0, inf)
print(math.modf(-math.inf))         # (-0.0, -inf)

# ── frexp ───────────────────────────────────────────────────────────────────
print(math.frexp(8.0))              # (0.5, 4)
print(math.frexp(0.0))              # (0.0, 0)
print(math.frexp(-0.0))             # (-0.0, 0)
print(math.frexp(1.0))              # (0.5, 1)
print(math.frexp(0.5))              # (0.5, 0)
print(math.frexp(-8.0))             # (-0.5, 4)
print(math.frexp(math.inf))         # (inf, 0)

# ── ldexp ───────────────────────────────────────────────────────────────────
print(math.ldexp(0.5, 4))           # 8.0
print(math.ldexp(1.0, 10))          # 1024.0
print(math.ldexp(1.0, -5))          # 0.03125
print(math.ldexp(0.0, 3))           # 0.0
print(math.ldexp(1.0, -10000))      # 0.0

try:
    math.ldexp(1.0, 10000)
except OverflowError as e:
    print("ldexp(1.0, 10000): OverflowError:", e)
try:
    math.ldexp(1.0, 2.5)
except TypeError as e:
    print("ldexp(1.0, 2.5): TypeError:", e)

# ── nextafter ───────────────────────────────────────────────────────────────
print(math.nextafter(1.0, 2.0))         # 1.0000000000000002
print(math.nextafter(1.0, 0.0))         # 0.9999999999999999
print(math.nextafter(1.0, 1.0))         # 1.0
print(math.nextafter(math.inf, 0.0))    # 1.7976931348623157e+308
print(math.nextafter(0.0, 1.0))         # 5e-324
print(math.nextafter(1.0, 2.0, steps=2))  # 1.0000000000000004
print(math.nextafter(1.0, 2.0, steps=0))  # 1.0
print(math.nextafter(1.0, 2.0, steps=1000))  # large step, stays finite
print(math.nextafter(0.0, -1.0))             # -5e-324 (cross zero)
print(math.nextafter(5e-324, -1.0, steps=2)) # -5e-324 (+smallest down 2)
print(math.nextafter(0.0, -0.0))             # -0.0 (equal: returns y)

try:
    math.nextafter(1.0, 2.0, steps=-1)
except ValueError as e:
    print("nextafter steps<0: ValueError:", e)
try:
    math.nextafter(1.0, 2.0, steps=2.5)
except TypeError as e:
    print("nextafter steps float: TypeError:", e)

# ── ulp ─────────────────────────────────────────────────────────────────────
print(math.ulp(1.0))                # 2.220446049250313e-16
print(math.ulp(0.0))                # 5e-324
print(math.ulp(-1.0))               # 2.220446049250313e-16
print(math.ulp(math.inf))           # inf
print(math.ulp(2.0))                # 4.440892098500626e-16

# ── isqrt ───────────────────────────────────────────────────────────────────
print(math.isqrt(17))               # 4
print(math.isqrt(16))               # 4
print(math.isqrt(0))                # 0
print(math.isqrt(1))                # 1
print(math.isqrt(10 ** 50))         # 100000...0 (exact bigint)
print(math.isqrt(2 ** 100))         # exact bigint
print(math.isqrt(True))             # 1  (bool is subclass of int)

try:
    math.isqrt(-1)
except ValueError as e:
    print("isqrt(-1): ValueError:", e)
try:
    math.isqrt(2.0)
except TypeError as e:
    print("isqrt(2.0): TypeError:", e)

# ── isclose ─────────────────────────────────────────────────────────────────
print(math.isclose(1.0, 1.0 + 1e-12))           # True
print(math.isclose(1.0, 1.1))                    # False
print(math.isclose(0.0, 0.0))                    # True
print(math.isclose(1e10, 1.00001e10))            # False
print(math.isclose(2.0, 2.0, abs_tol=0.5))       # True
print(math.isclose(1.0, 1.05, abs_tol=0.1))      # True
print(math.isclose(math.inf, math.inf))          # True
print(math.isclose(math.nan, math.nan))          # False

try:
    math.isclose(1.0, 1.0, rel_tol=-1.0)
except ValueError as e:
    print("isclose rel_tol<0: ValueError:", e)
