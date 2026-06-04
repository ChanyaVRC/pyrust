# Parity tests for the math functions added in issue #2179:
# fsum, sumprod, gamma, lgamma, erf, erfc — plus their domain / edge cases.
# Output is diffed against CPython 3.12, so every printed value must match to
# the last ULP.
import math

# ── fsum: exactly-rounded summation ─────────────────────────────────────────
print(math.fsum([0.1] * 10))            # 1.0 (naive sum gives 0.9999999999999999)
print(math.fsum([1e16, 1, -1e16]))      # 1.0
print(math.fsum([1, 1e100, 1, -1e100])) # 2.0
print(math.fsum([]))                    # 0.0
print(math.fsum([0.0]))                 # 0.0
print(math.fsum([-0.0]))                # 0.0
print(math.fsum([1.0, 2.0, 3.0]))       # 6.0
print(math.fsum([0.1] * 100))           # 10.0
print(math.fsum(range(1, 1001)))        # 500500.0 (ints accepted)
print(math.fsum([True, 0.5, 2]))        # 3.5 (bool/int coerced)
print(math.fsum([math.pi, math.e, -math.pi, -math.e]))  # 0.0

# fsum special / error cases
print(math.fsum([float("inf"), 1.0]))           # inf
print(math.fsum([float("nan"), 1.0]))           # nan
try:
    math.fsum([float("inf"), float("-inf")])    # -inf + inf -> ValueError
except ValueError as e:
    print("ValueError", e)
try:
    math.fsum([1e308, 1e308, -1e308])           # intermediate overflow
except OverflowError as e:
    print("OverflowError", e)

# ── sumprod: sum of products ────────────────────────────────────────────────
print(math.sumprod([1, 2, 3], [4, 5, 6]))       # 32 (int)
print(math.sumprod([1.0, 2.0, 3.0], [4.0, 5.0, 6.0]))  # 32.0 (float)
print(math.sumprod([], []))                     # 0 (int)
print(math.sumprod([1.5, 2.5], [2.0, 4.0]))     # 13.0
print(math.sumprod([True, False], [5, 7]))      # 5
print(math.sumprod([10**20, 2], [3, 10**19]))   # exact big int
print(math.sumprod([1e308], [2.0]))             # inf (overflow fallback)
try:
    math.sumprod([1, 2], [3, 4, 5])             # length mismatch
except ValueError as e:
    print("ValueError", e)

# ── gamma ───────────────────────────────────────────────────────────────────
print(math.gamma(5))                    # 24.0
print(math.gamma(0.5) == math.sqrt(math.pi))  # True
print(math.gamma(1.0))                  # 1.0
print(math.gamma(2.0))                  # 1.0
print(math.gamma(10.0))                 # 362880.0
print(math.gamma(0.1))
print(math.gamma(-0.5))
print(math.gamma(-1.5))
print(repr(math.gamma(float("inf"))))   # inf
print(repr(math.gamma(float("nan"))))   # nan
for x in [0.0, -0.0, -1.0, -2.0]:
    try:
        math.gamma(x)
    except ValueError as e:
        print("gamma domain", e)
try:
    math.gamma(200.0)
except OverflowError as e:
    print("gamma overflow", e)

# ── lgamma ──────────────────────────────────────────────────────────────────
print(math.lgamma(1.0))                 # 0.0
print(math.lgamma(2.0))                 # 0.0
print(math.lgamma(5.0))                 # 3.178053830347945
print(math.lgamma(0.5))
print(math.lgamma(-0.5))
print(math.lgamma(100.0))
print(repr(math.lgamma(float("inf"))))  # inf
print(repr(math.lgamma(float("nan"))))  # nan
for x in [0.0, -1.0, -5.0]:
    try:
        math.lgamma(x)
    except ValueError as e:
        print("lgamma domain", e)

# ── erf / erfc ──────────────────────────────────────────────────────────────
print(math.erf(0.0))                    # 0.0
print(math.erf(1.0))                    # 0.8427007929497149
print(math.erf(-1.0))
print(math.erf(2.0))
print(math.erf(float("inf")))           # 1.0
print(math.erf(float("-inf")))          # -1.0
print(repr(math.erf(float("nan"))))     # nan
print(math.erfc(0.0))                   # 1.0
print(math.erfc(1.0))                   # 0.15729920705028513
print(math.erfc(-1.0))
print(math.erfc(2.0))
print(math.erfc(float("inf")))          # 0.0
print(math.erfc(float("-inf")))         # 2.0
print(repr(math.erfc(float("nan"))))    # nan

# erf + erfc complementarity
print(math.erf(0.7) + math.erfc(0.7) == 1.0)  # True

# ── numeric protocol acceptance (#1933): __float__ / __index__ ──────────────
class MyFloat:
    def __float__(self):
        return 2.0

class MyIndex:
    def __index__(self):
        return 3

print(math.gamma(MyFloat()))            # gamma(2.0) == 1.0
print(math.erf(MyIndex()))              # erf(3)
print(math.fsum([MyFloat(), MyFloat()]))  # 4.0
# sumprod uses the `*`/`+` operators for non-numeric elements, so an object
# without __mul__ raises TypeError (it does NOT consult __float__).
try:
    math.sumprod([MyFloat()], [MyFloat()])
except TypeError:
    print("sumprod TypeError")
# but sumprod of plain ints/floats with a __mul__-capable object works via `*`
print(math.sumprod([2.0, 3.0], [4, 5]))  # 22.0 (float/int pairs)
