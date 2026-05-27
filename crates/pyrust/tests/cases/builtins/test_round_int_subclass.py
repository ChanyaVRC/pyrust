class MyInt(int):
    pass


# Basic int subclass rounding — inherits int.__round__ behaviour.
print(round(MyInt(7)))
print(round(MyInt(7), 0))
print(round(MyInt(7), -1))
print(round(MyInt(25), -1))   # banker's rounding: 25 → 20 (round half to even)
print(round(MyInt(1234), -2))

# Plain int regression guard (must not regress from #1417 fix).
print(round(7))
print(round(7, 2))
print(round(42, -2))
print(round(1234, -2))

# User-defined __round__ takes priority over inherited int rounding.
class MyIntWithRound(int):
    def __round__(self, ndigits=None):
        return "custom_" + str(ndigits)


print(round(MyIntWithRound(5)))
print(round(MyIntWithRound(5), 1))

# Float subclass inherits float.__round__ behaviour.
class MyFloat(float):
    pass


print(round(MyFloat(3.5)))
print(round(MyFloat(2.5)))
print(round(MyFloat(3.14), 1))

# Non-numeric type still raises TypeError.
try:
    round("x")
except TypeError as e:
    print(type(e).__name__ + ":", e)

# Explicit None ndigits is the same as omitting ndigits.
print(round(MyInt(42), None))
