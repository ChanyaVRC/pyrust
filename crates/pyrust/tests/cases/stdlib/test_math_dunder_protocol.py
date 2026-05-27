# Parity tests for math.floor / math.ceil / math.trunc dunder dispatch.
# CPython 3.12: these functions first try type(x).__floor__ / __ceil__ /
# __trunc__ before falling back to float coercion.
# See: https://docs.python.org/3/library/math.html#math.floor
import math


# ── Custom class with all three dunders ──────────────────────────────────────

class MyNum:
    def __floor__(self): return 42
    def __ceil__(self): return 99
    def __trunc__(self): return 7

print(math.floor(MyNum()))   # 42
print(math.ceil(MyNum()))    # 99
print(math.trunc(MyNum()))   # 7


# ── Fraction-like class (the motivating use case) ────────────────────────────

class Frac:
    def __init__(self, n, d):
        self.n = n
        self.d = d
    def __floor__(self):
        return self.n // self.d
    def __ceil__(self):
        return -((-self.n) // self.d)
    def __trunc__(self):
        return int(self.n / self.d)

f = Frac(7, 2)
print(math.floor(f))    # 3
print(math.ceil(f))     # 4
print(math.trunc(f))    # 3


# ── __floor__ / __ceil__ returning non-int (CPython forwards unchanged) ──────

class ReturnFloat:
    def __floor__(self): return 3.5
    def __ceil__(self): return 4.5
    def __trunc__(self): return 2.5

print(math.floor(ReturnFloat()))    # 3.5
print(math.ceil(ReturnFloat()))     # 4.5
print(math.trunc(ReturnFloat()))    # 2.5


# ── Primitive types — float fallback path ────────────────────────────────────

print(math.floor(3.7))     # 3
print(math.floor(-3.2))    # -4
print(math.ceil(3.2))      # 4
print(math.ceil(-3.7))     # -3
print(math.trunc(3.7))     # 3
print(math.trunc(-3.7))    # -3

# int and bool: trunc returns unchanged; floor/ceil return unchanged
print(math.floor(5))       # 5
print(math.ceil(5))        # 5
print(math.trunc(5))       # 5
print(math.trunc(-5))      # -5
print(math.floor(True))    # 1
print(math.ceil(False))    # 0
print(math.trunc(True))    # 1


# ── BigInt input for floor / ceil / trunc ──────────────────────────────────
# int.__floor__ and int.__ceil__ return self unchanged, so large ints that
# cannot be represented exactly as f64 must not be coerced to float first.

big = 2**63 + 1
print(math.trunc(big))     # 9223372036854775809
print(math.floor(big))     # 9223372036854775809
print(math.ceil(big))      # 9223372036854775809

# 2**53 + 1 is the smallest int that can't be represented exactly as f64.
# math.floor / math.ceil must return it unchanged, not 2**53.
precise = 2**53 + 1
print(math.floor(precise))   # 9007199254740993
print(math.ceil(precise))    # 9007199254740993


# ── Error: custom class without __trunc__ ────────────────────────────────────

class NoTrunc: pass

try:
    math.trunc(NoTrunc())
except TypeError as e:
    print("TypeError:", e)    # type NoTrunc doesn't define __trunc__ method


# ── Error: custom class without __floor__ / __ceil__ ────────────────────────

class NoFloorCeil: pass

try:
    math.floor(NoFloorCeil())
except TypeError as e:
    print("TypeError:", e)    # must be real number, not NoFloorCeil

try:
    math.ceil(NoFloorCeil())
except TypeError as e:
    print("TypeError:", e)    # must be real number, not NoFloorCeil


# ── Error: str type ──────────────────────────────────────────────────────────

try:
    math.trunc("hello")
except TypeError as e:
    print("TypeError:", e)    # type str doesn't define __trunc__ method

try:
    math.floor("hello")
except TypeError as e:
    print("TypeError:", e)    # must be real number, not str

try:
    math.ceil("hello")
except TypeError as e:
    print("TypeError:", e)    # must be real number, not str
