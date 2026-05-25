import math

# Basic non-zero cases
print(7.0 % 3.0)    # 1.0
print(-7.0 % 3.0)   # 2.0
print(7.0 % -3.0)   # -2.0
print(-7.0 % -3.0)  # -1.0

# Zero result cases: sign of result must match sign of divisor
print(repr(0.0 % 3.0))     # 0.0
print(repr(0.0 % -3.0))    # -0.0
print(repr(6.0 % 3.0))     # 0.0
print(repr(6.0 % -3.0))    # -0.0

# Confirm sign via copysign
print(math.copysign(1, 0.0 % 3.0))    # 1.0
print(math.copysign(1, 0.0 % -3.0))   # -1.0

# ZeroDivisionError
try:
    print(1.0 % 0.0)
except ZeroDivisionError as e:
    print(type(e).__name__)
