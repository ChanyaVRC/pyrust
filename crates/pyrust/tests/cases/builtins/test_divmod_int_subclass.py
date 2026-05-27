# Parity fixture for divmod() with int/float subclass instances.
# Issue #1433: divmod(MyInt(10), 3) raised TypeError instead of (3, 1).
# CPython delegates through nb_divmod inherited from int/float when the
# subclass does not define its own __divmod__.


class MyInt(int):
    pass


class MyFloat(float):
    pass


# ── Basic int subclass cases ──────────────────────────────────────────────────

print(divmod(MyInt(10), 3))          # (3, 1)
print(divmod(MyInt(10), 3.0))        # (3.0, 1.0)
print(divmod(3, MyInt(2)))           # (1, 1)
print(divmod(MyInt(7), MyInt(3)))    # (2, 1)

# ── Basic float subclass cases ────────────────────────────────────────────────

print(divmod(MyFloat(2.5), 1.5))     # (1.0, 1.0)
print(divmod(1.5, MyFloat(0.5)))     # (3.0, 0.0)
print(divmod(MyFloat(7.0), MyInt(3)))  # (2.0, 1.0)

# ── Negative values ───────────────────────────────────────────────────────────

print(divmod(MyInt(-7), 3))          # (-3, 2)
print(divmod(MyInt(7), -3))          # (-3, -2)

# ── Zero-division raises correctly ────────────────────────────────────────────

try:
    divmod(MyInt(10), MyInt(0))
except ZeroDivisionError as e:
    print("ZeroDivisionError:", e)

try:
    divmod(MyFloat(1.0), MyFloat(0.0))
except ZeroDivisionError as e:
    print("ZeroDivisionError:", e)

# ── Primitive divmod still works (regression guard) ───────────────────────────

print(divmod(5, 2))                  # (2, 1)
print(divmod(5.5, 2.5))             # (2.0, 0.5)

# ── Custom __divmod__ on a subclass takes priority over coercion ──────────────

class WithDivmod(int):
    def __divmod__(self, other):
        return ("custom", "divmod")


print(divmod(WithDivmod(10), 3))     # ('custom', 'divmod')

# ── bool mixed with int subclass ──────────────────────────────────────────────

print(divmod(True, MyInt(3)))        # (0, 1)  — bool is subtype of int
print(divmod(MyInt(5), True))        # (5, 0)

# ── TypeError still raised for non-numeric operands ──────────────────────────

try:
    divmod(MyInt(10), "hello")
except TypeError:
    print("TypeError")

# ── __rdivmod__ returning NotImplemented falls through to coercion ────────────

class NotImplRDivmod(int):
    def __rdivmod__(self, other):
        return NotImplemented


print(divmod(MyInt(10), NotImplRDivmod(3)))  # (3, 1)
