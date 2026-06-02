# Parity tests for math float/int argument coercion via the numeric protocols.
# CPython 3.12: math's float-taking functions accept objects with __float__
# (preferred) or __index__, plus float/int subclasses; int-taking functions
# (gcd/lcm/comb/perm/factorial/isqrt) accept __index__ and int subclasses but
# reject __float__-only objects.
# See: https://docs.python.org/3/library/math.html
import math


class WF:
    def __float__(self): return 16.0


class WI:
    def __index__(self): return 16


class F(float):
    pass


class I(int):
    pass


class Both:
    # __float__ is preferred over __index__ (CPython nb_float before nb_index).
    def __float__(self): return 9.0
    def __index__(self): return 25


def show(label, fn):
    try:
        print(label, fn())
    except Exception as e:
        print(label, type(e).__name__, str(e))


# ── Float-taking functions accept __float__ / __index__ / subclasses ─────────
show("sqrt WF", lambda: math.sqrt(WF()))        # 4.0
show("sqrt WI", lambda: math.sqrt(WI()))        # 4.0  (math accepts __index__)
show("sqrt F", lambda: math.sqrt(F(16.0)))      # 4.0  (float subclass)
show("sqrt I", lambda: math.sqrt(I(16)))        # 4.0  (int subclass)
show("sin WF", lambda: math.sin(WF()))
show("cos WI", lambda: math.cos(WI()))
show("fabs F", lambda: math.fabs(F(-3.0)))      # 3.0
show("isnan F", lambda: math.isnan(F(1.0)))     # False
show("isinf WF", lambda: math.isinf(WF()))      # False
show("isfinite WI", lambda: math.isfinite(WI()))
show("log F", lambda: math.log(F(2.718281828459045)))

# Precedence: __float__ wins over __index__.
show("both float-wins", lambda: math.sqrt(Both()))   # 3.0, not 5.0

# Two-argument float functions accept protocol objects in BOTH args.
show("pow WF WI", lambda: math.pow(WF(), WI()))
show("log x base", lambda: math.log(WF(), WI()))
show("atan2 WF WI", lambda: math.atan2(WF(), WI()))
show("copysign WF WI", lambda: math.copysign(WF(), WI()))
show("hypot WF WI", lambda: math.hypot(WF(), WI()))

# ── Int-taking functions accept __index__ / int subclass, reject __float__ ───
show("gcd I 6", lambda: math.gcd(I(12), 6))     # 6
show("gcd WI 24", lambda: math.gcd(WI(), 24))   # 8
show("lcm WI 6", lambda: math.lcm(WI(), 6))     # 48
show("factorial I", lambda: math.factorial(I(5)))    # 120
show("comb WI 2", lambda: math.comb(WI(), 2))        # 120
show("perm WI 2", lambda: math.perm(WI(), 2))        # 240
show("isqrt I", lambda: math.isqrt(I(16)))           # 4

# __float__-only object is NOT an integer for int-taking functions.
show("gcd WF", lambda: math.gcd(WF(), 6))
show("factorial WF", lambda: math.factorial(WF()))
show("isqrt WF", lambda: math.isqrt(WF()))

# ── Concrete types unchanged ─────────────────────────────────────────────────
show("sqrt 16", lambda: math.sqrt(16))          # 4.0
show("sqrt 16.0", lambda: math.sqrt(16.0))      # 4.0
show("sqrt True", lambda: math.sqrt(True))      # 1.0
show("gcd 12 8", lambda: math.gcd(12, 8))       # 4


# ── Error parity ─────────────────────────────────────────────────────────────

class BadF:
    def __float__(self): return "x"


class IntF:
    def __float__(self): return 5


class BoolF:
    def __float__(self): return True


class BadI:
    def __index__(self): return 1.5


show("sqrt obj", lambda: math.sqrt(object()))         # must be real number, not object
show("sqrt BadF", lambda: math.sqrt(BadF()))          # __float__ returned non-float (str)
show("sqrt IntF", lambda: math.sqrt(IntF()))          # __float__ returned non-float (int)
show("sqrt BoolF", lambda: math.sqrt(BoolF()))        # __float__ returned non-float (bool)
show("gcd BadI", lambda: math.gcd(BadI(), 4))         # __index__ returned non-int (float)
show("gcd obj", lambda: math.gcd(object(), 4))        # object cannot be interpreted as int


# ── Subclass dunder-override precedence (CPython PyFloat_AsDouble quirks) ─────

class FOver(float):
    # A float subclass overriding __float__: math IGNORES it and uses the
    # backing value (PyFloat_Check fast path).
    def __float__(self): return 100.0


class IBoth(int):
    # An int subclass overriding __float__: math uses the override.
    def __index__(self): return 100
    def __float__(self): return 100.0


class IIdx(int):
    # An int subclass overriding only __index__: math uses the inherited
    # int.__float__ (backing), NOT the __index__ override.
    def __index__(self): return 100


show("sqrt FOver", lambda: math.sqrt(FOver(16.0)))    # 4.0  (backing, not 100)
show("sqrt IBoth", lambda: math.sqrt(IBoth(16)))      # 10.0 (__float__ override)
show("sqrt IIdx", lambda: math.sqrt(IIdx(16)))        # 4.0  (backing, not __index__)
show("gcd IBoth", lambda: math.gcd(IBoth(16), 24))    # 8    (backing 16, not 100)
show("gcd IIdx", lambda: math.gcd(IIdx(16), 24))      # 8    (backing 16, not 100)
