# Parity fixture: pow() builtin dispatches __pow__ / __rpow__ for user-defined types.
# Issue #1264: pow(obj, exp) was raising TypeError instead of calling __pow__.

# 2-arg form: __pow__ dispatch
class MyNum:
    def __init__(self, v): self.v = v
    def __pow__(self, other, mod=None):
        other_v = other.v if isinstance(other, MyNum) else other
        if mod is not None:
            return MyNum(pow(self.v, other_v, mod))
        return MyNum(self.v ** other_v)
    def __repr__(self): return "MyNum(" + str(self.v) + ")"

x = MyNum(3)
print(pow(x, 2))          # MyNum(9)
print(x ** 2)             # MyNum(9)  -- operator path (should already work)

# 3-arg form: __pow__(exp, mod) dispatch
print(pow(x, 2, 5))       # MyNum(4)  -- 3**2 mod 5 = 4

# __rpow__ dispatch: pow(int, user_obj) calls user_obj.__rpow__(int)
class WithRpow:
    def __init__(self, n): self.n = n
    def __rpow__(self, base):
        return WithRpow(base ** self.n)
    def __repr__(self): return "WR(" + str(self.n) + ")"

print(pow(2, WithRpow(3)))   # WR(8)
print(pow(5, WithRpow(2)))   # WR(25)

# Built-in int fast path unaffected
print(pow(2, 3))             # 8
print(pow(2, 10, 100))       # 24
print(pow(0, 0))             # 1

# Float pow unaffected
import math
assert abs(pow(2.0, 0.5) - math.sqrt(2)) < 1e-10
print("float pow ok")

# __pow__ returning NotImplemented falls through to TypeError
class NoPow:
    def __pow__(self, exp, mod=None): return NotImplemented

try:
    pow(NoPow(), 2)
except TypeError as e:
    print("NoPow TypeError:", e)

print("done")
