# Parity fixture for round() __round__ dunder dispatch (issue #1095).
# round(obj) with no ndigits calls obj.__round__() with no arguments.
# round(obj, n) calls obj.__round__(n).
# Objects without __round__ raise TypeError.

class MyNum:
    def __init__(self, x):
        self.x = x

    def __round__(self, ndigits=None):
        if ndigits is None:
            return round(self.x)
        return round(self.x, ndigits)


# Basic __round__ dispatch
print(round(MyNum(3.14159)))      # 3
print(round(MyNum(3.14159), 2))   # 3.14
print(round(MyNum(3.14159), -1))  # 0.0

# Negative ndigits
print(round(MyNum(1234.5), -2))   # 1200.0

# __round__ can return any type (not just numeric)
class Quirky:
    def __round__(self, ndigits=None):
        return "rounded" if ndigits is None else f"rounded({ndigits})"

print(round(Quirky()))            # rounded
print(round(Quirky(), 3))         # rounded(3)

# Primitive types still work
print(round(42))                  # 42
print(round(3.14, 2))             # 3.14
print(round(True))                # 1
print(round(False))               # 0

# Missing __round__ raises TypeError
class NoRound:
    pass

try:
    round(NoRound())
except TypeError:
    print("TypeError")

# ndigits=None explicitly is the same as absent — __round__() called with no args
print(round(MyNum(2.5), None))    # 2  (banker's rounding of 2.5)

# ndigits as bool: True == 1, False == 0 — forwarded as bool to __round__
class ShowNdigits:
    def __round__(self, ndigits=None):
        return type(ndigits).__name__

print(round(ShowNdigits(), True))   # bool
print(round(ShowNdigits(), False))  # bool
print(round(ShowNdigits(), None))   # NoneType — no arg → default → None
print(round(ShowNdigits()))         # NoneType

# Inherited __round__ via class chain
class Base:
    def __round__(self, ndigits=None):
        return 99 if ndigits is None else 100

class Child(Base):
    pass

print(round(Child()))             # 99
print(round(Child(), 0))          # 100
