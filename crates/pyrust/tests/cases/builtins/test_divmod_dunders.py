# Parity fixture for divmod() dunder protocol (__divmod__ / __rdivmod__).
# Issue #1094: the catch-all overload previously raised TypeError immediately
# without consulting the dunder protocol, diverging from CPython's
# PyNumber_Divmod which tries __divmod__ on the left operand first, then
# __rdivmod__ on the right operand.


# ── Basic __divmod__ ──────────────────────────────────────────────────────────

class MyNum:
    def __init__(self, x):
        self.x = x

    def __divmod__(self, other):
        if isinstance(other, MyNum):
            return (self.x // other.x, self.x % other.x)
        return NotImplemented

    def __rdivmod__(self, other):
        if isinstance(other, int):
            return (other // self.x, other % self.x)
        return NotImplemented


# __divmod__ called on left operand when both are MyNum.
print(divmod(MyNum(10), MyNum(3)))   # (3, 1)
print(divmod(MyNum(7), MyNum(3)))    # (2, 1)
print(divmod(MyNum(0), MyNum(5)))    # (0, 0)

# __rdivmod__ called on right operand when left is int.
print(divmod(10, MyNum(3)))          # (3, 1)
print(divmod(7, MyNum(3)))           # (2, 1)


# ── NotImplemented fallback to __rdivmod__ ────────────────────────────────────

class Left:
    def __divmod__(self, other):
        return NotImplemented   # always declines

class Right:
    def __rdivmod__(self, other):
        # Use type name to keep output deterministic across runs.
        return ("rdivmod", type(other).__name__)

print(divmod(Left(), Right()))       # ('rdivmod', 'Left')


# ── No dunders → TypeError ────────────────────────────────────────────────────

class NoDiv:
    pass

try:
    divmod(NoDiv(), NoDiv())
except TypeError as e:
    print("TypeError:", e)

try:
    divmod(NoDiv(), 1)
except TypeError as e:
    print("TypeError:", e)


# ── Primitive paths still work ────────────────────────────────────────────────

print(divmod(10, 3))                 # (3, 1)
print(divmod(10.5, 3.0))            # (3.0, 1.5)
print(divmod(True, 2))              # (0, 1)


# ── Subtype rule: b.__rdivmod__ tried first when b is a proper subtype of a ───

class Base:
    def __divmod__(self, other):
        return ("base_divmod", type(other).__name__)

    def __rdivmod__(self, other):
        return ("base_rdivmod", type(other).__name__)


class Sub(Base):
    def __rdivmod__(self, other):
        return ("sub_rdivmod", type(other).__name__)


# Sub is a proper subtype of Base: Sub.__rdivmod__ is tried first.
print(divmod(Base(), Sub()))         # ('sub_rdivmod', 'Base')
# Sub as left operand: __divmod__ (inherited from Base) is tried first.
print(divmod(Sub(), Base()))         # ('base_divmod', 'Base')
# Same type: normal left-first order applies.
print(divmod(Base(), Base()))        # ('base_divmod', 'Base')

# Subtype's __rdivmod__ returns NotImplemented: falls back to a.__divmod__.
class Sub2(Base):
    def __rdivmod__(self, other):
        return NotImplemented

print(divmod(Base(), Sub2()))        # ('base_divmod', 'Sub2')
