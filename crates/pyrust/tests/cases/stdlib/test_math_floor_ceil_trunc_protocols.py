import math


# --- __index__-only / __float__-only fallback (floor/ceil) ---
class WI:
    def __index__(self):
        return 7


class WF:
    def __float__(self):
        return 7.8


print(math.floor(WI()), type(math.floor(WI())).__name__)  # 7 int (via __index__)
print(math.ceil(WI()))   # 7
print(math.floor(WF()))  # 7
print(math.ceil(WF()))   # 8  (rounds up)


# __float__ wins over __index__ for floor/ceil (matches PyFloat_AsDouble order)
class WB:
    def __index__(self):
        return 7

    def __float__(self):
        return 7.8


print(math.floor(WB()))  # 7  (floor(7.8))
print(math.ceil(WB()))   # 8  (ceil(7.8))


# --- dedicated dunder wins, and is consulted per-op ---
class WBoth:
    def __floor__(self):
        return 100

    def __float__(self):
        return 7.8


print(math.floor(WBoth()))  # 100 (dedicated __floor__)
print(math.ceil(WBoth()))   # 8   (no __ceil__ -> falls back to __float__)


# --- trunc does NOT fall back to __index__/__float__ on plain objects ---
for obj in (WI(), WF(), WB()):
    try:
        math.trunc(obj)
    except TypeError as e:
        print("trunc plain:", e)


# trunc still uses a dedicated __trunc__
class WTrunc:
    def __trunc__(self):
        return -9

    def __float__(self):
        return 2.9


print(math.trunc(WTrunc()))  # -9


# --- int / float subclasses ---
class I(int):
    pass


class F(float):
    pass


print(math.floor(I(5)), math.ceil(I(5)), math.trunc(I(5)))      # 5 5 5
print(math.floor(F(3.7)), math.ceil(F(3.7)), math.trunc(F(3.7)))  # 3 4 3

# int subclass: exact value preserved (no f64 round-trip)
big = I(2**60 + 1)
print(math.floor(big) == 2**60 + 1, math.trunc(big) == 2**60 + 1)  # True True

# int subclass with a __float__ override: floor/ceil/trunc use the int backing
class IF(int):
    def __float__(self):
        return 999.9


print(math.floor(IF(5)), math.ceil(IF(5)), math.trunc(IF(5)))  # 5 5 5

# float subclass with __index__ override: uses the float backing
class FX(float):
    def __index__(self):
        return 1


print(math.floor(FX(3.7)), math.ceil(FX(3.7)))  # 3 4


# --- concrete numbers unchanged ---
print(math.floor(3.7), math.ceil(3.2), math.trunc(-3.7))  # 3 4 -3
print(math.floor(True), math.ceil(False), math.trunc(True))  # 1 0 1


# --- error paths ---
class N:
    pass


for fn in (math.floor, math.ceil):
    try:
        fn(N())
    except TypeError as e:
        print(fn.__name__, "N:", e)

try:
    math.trunc(N())
except TypeError as e:
    print("trunc N:", e)


# __float__ returning a non-float, __index__ returning a non-int
class BadF:
    def __float__(self):
        return "x"


class BadI:
    def __index__(self):
        return 1.5


try:
    math.floor(BadF())
except TypeError as e:
    print("BadF:", e)

try:
    math.floor(BadI())
except TypeError as e:
    print("BadI:", e)


# nan / inf via __float__
class NanF:
    def __float__(self):
        return float("nan")


class InfF:
    def __float__(self):
        return float("inf")


try:
    math.floor(NanF())
except ValueError as e:
    print("NaN:", e)

try:
    math.ceil(InfF())
except OverflowError as e:
    print("Inf:", e)
