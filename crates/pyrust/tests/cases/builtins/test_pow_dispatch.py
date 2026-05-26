# Parity fixture for pow() dispatching __pow__ / __rpow__ on user-defined types.
# Covers issue #1264: pow(obj, exp) was raising TypeError instead of calling __pow__.

# ── Basic __pow__ dispatch ────────────────────────────────────────────────────

class MyNum:
    def __init__(self, n): self.n = n
    def __pow__(self, exp, mod=None):
        if mod is not None:
            return MyNum(pow(self.n, exp, mod))
        return MyNum(self.n ** exp)
    def __repr__(self): return f"MyNum({self.n})"

print(pow(MyNum(2), 3))       # MyNum(8)
print(pow(MyNum(3), 3))       # MyNum(27)

# 3-arg pow dispatches __pow__(exp, mod)
print(pow(MyNum(2), 3, 5))    # MyNum(3)  — 2^3 mod 5 = 3
print(pow(MyNum(10), 2, 7))   # MyNum(2)  — 10^2 mod 7 = 2

# __ operator is unaffected (should already work)
print(MyNum(2) ** 4)          # MyNum(16)

# ── __pow__ returning NotImplemented ─────────────────────────────────────────

class PartialPow:
    def __pow__(self, exp, mod=None): return NotImplemented

try:
    pow(PartialPow(), 2)
except TypeError as e:
    print("no __pow__ TypeError:", e)

# ── __rpow__ dispatch ─────────────────────────────────────────────────────────

class WithRpow:
    def __init__(self, n): self.n = n
    def __rpow__(self, base, mod=None): return f"rpow({base},{self.n})"

# int ** WithRpow → __rpow__ is called
print(pow(2, WithRpow(3)))    # rpow(2,3)
print(pow(5, WithRpow(2)))    # rpow(5,2)

# ── Same type: only __pow__, NOT __rpow__ ────────────────────────────────────

class SameType:
    def __pow__(self, exp, mod=None):
        print("SameType.__pow__ called")
        return NotImplemented
    def __rpow__(self, base, mod=None):
        print("SameType.__rpow__ called")
        return "rpow"

a = SameType()
try:
    result = pow(a, a)
    print("result:", result)
except TypeError as e:
    print("same-type TypeError:", e)

# ── Subtype rule: proper subtype's __rpow__ tried first ───────────────────────

class Base:
    def __pow__(self, exp, mod=None):
        print("Base.__pow__")
        return NotImplemented

class Sub(Base):
    def __rpow__(self, base, mod=None):
        print("Sub.__rpow__")
        return "sub_rpow"

b = Base()
s = Sub()
print(pow(b, s))   # Sub.__rpow__ tried first (Sub is proper subtype of Base)

# ── Built-in pow still works ──────────────────────────────────────────────────

assert pow(2, 10) == 1024
assert pow(2, 10, 100) == 24
assert pow(0, 0) == 1
assert pow(-2, 3) == -8

# Float pow
import math
assert abs(pow(2.0, 0.5) - math.sqrt(2)) < 1e-10

# 3-arg TypeError for non-integers
try:
    pow(2.0, 3, 5)
except TypeError as e:
    print("pow(2.0,3,5) TypeError:", e)

try:
    pow(2, 3.0, 5)
except TypeError as e:
    print("pow(2,3.0,5) TypeError:", e)

# 3-arg ValueError: modulus 0
try:
    pow(2, 3, 0)
except ValueError as e:
    print("pow(2,3,0) ValueError:", e)

print("pow dispatch OK")
