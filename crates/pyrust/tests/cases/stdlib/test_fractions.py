from fractions import Fraction

# Construction
print(Fraction(1, 3))                  # 1/3
print(Fraction(2, 4))                  # 1/2
print(Fraction(3))                     # 3
print(Fraction(4, 2))                  # 2
print(Fraction('1/3'))                 # 1/3
print(Fraction('0.5'))                 # 1/2
print(Fraction('-35/4'))               # -35/4
print(Fraction('-47e-2'))              # -47/100
print(Fraction(0.5))                   # 1/2
print(Fraction())                      # 0
print(repr(Fraction(1, 3)))            # Fraction(1, 3)
print(repr(Fraction(4, 2)))            # Fraction(2, 1)
print(Fraction(0.1))                   # 3602879701896397/36028797018963968

# Negative normalization
print(Fraction(1, -3))                 # -1/3
print(Fraction(-2, -4))                # 1/2

# Arithmetic
print(Fraction(1, 3) + Fraction(1, 6)) # 1/2
print(Fraction(1, 3) - Fraction(1, 6)) # 1/6
print(Fraction(1, 3) * 3)             # 1
print(Fraction(1, 3) / Fraction(1, 6))# 2
print(Fraction(2, 3) ** 2)            # 4/9
print(Fraction(2, 3) ** -1)           # 3/2
print(Fraction(7, 3) // Fraction(1, 3))  # 7
print(Fraction(7, 3) % Fraction(1, 2))   # 1/3
print(divmod(Fraction(7, 3), Fraction(1, 2)))  # (4, Fraction(1, 3))
print(-Fraction(1, 3))                 # -1/3
print(abs(Fraction(-1, 3)))            # 1/3

# Mixed-type arithmetic
print(Fraction(1, 2) + 1)             # 3/2
print(1 + Fraction(1, 2))             # 3/2
print(2 * Fraction(1, 3))             # 2/3
print(Fraction(1, 2) + 0.5)           # 1.0
print(Fraction(1, 4) ** 0.5)          # 0.5

# Attributes
f = Fraction(3, 4)
print(f.numerator)                     # 3
print(f.denominator)                   # 4
print(Fraction(6, 4).numerator)        # 3

# Comparisons
print(Fraction(1, 3) < Fraction(1, 2)) # True
print(Fraction(1, 2) == 0.5)           # True
print(Fraction(1, 3) == Fraction(1, 3))# True
print(Fraction(1, 2) > Fraction(1, 3)) # True
print(Fraction(1, 2) == Fraction(2, 4))# True
print(Fraction(2, 1) == 2)             # True
print(sorted([Fraction(1, 2), Fraction(1, 3), Fraction(1, 4)]))

# Conversions
print(int(Fraction(7, 2)))             # 3
print(float(Fraction(1, 4)))           # 0.25
print(bool(Fraction(0)))               # False
print(bool(Fraction(1, 2)))            # True

# From float / from_float
print(Fraction.from_float(0.25))       # 1/4
print(Fraction.from_float(2))          # 2

# is_integer / as_integer_ratio
print(Fraction(4, 2).is_integer())     # True
print(Fraction(3, 2).is_integer())     # False
print(Fraction(3, 4).as_integer_ratio())  # (3, 4)

# limit_denominator
print(Fraction('3.14159265').limit_denominator(100))  # 311/99
print(Fraction('3.14159265').limit_denominator(10))   # 22/7
print(Fraction(4321, 8765).limit_denominator(10000))  # 4321/8765

# round
print(round(Fraction(7, 2)))           # 4
print(round(Fraction(5, 2)))           # 2
print(round(Fraction(1, 3), 2))        # 33/100

# hash agrees with int and float
print(hash(Fraction(3, 1)) == hash(3))       # True
print(hash(Fraction(1, 2)) == hash(0.5))     # True

# str vs repr of denominator-1
print(str(Fraction(1, 1)))             # 1
print(repr(Fraction(1, 1)))            # Fraction(1, 1)

# Zero denominator raises
try:
    Fraction(1, 0)
except ZeroDivisionError:
    print("zero denom ok")             # zero denom ok

# Division by zero fraction raises
try:
    Fraction(1, 2) / Fraction(0)
except ZeroDivisionError:
    print("zero div ok")               # zero div ok

# Invalid string raises
try:
    Fraction('not a number')
except ValueError:
    print("bad string ok")             # bad string ok

print("fractions ok")
