"""
int() and math.floor/ceil/trunc must raise for inf and NaN inputs.

CPython 3.12 reference:
  int(float('inf'))  → OverflowError: cannot convert float infinity to integer
  int(float('nan'))  → ValueError:   cannot convert float NaN to integer
  math.floor(inf)    → OverflowError: cannot convert float infinity to integer
  math.floor(nan)    → ValueError:   cannot convert float NaN to integer
  math.ceil(inf)     → OverflowError: cannot convert float infinity to integer
  math.ceil(nan)     → ValueError:   cannot convert float NaN to integer
  math.trunc(inf)    → OverflowError: cannot convert float infinity to integer
  math.trunc(nan)    → ValueError:   cannot convert float NaN to integer
"""

import math

# Happy paths — must still work.
print(int(1.9))
print(int(-1.9))
print(int(0.0))
print(int(-0.0))
print(math.floor(1.7))
print(math.ceil(1.2))
print(math.trunc(3.9))

# int() with inf/nan — must raise.
try:
    int(float('inf'))
except OverflowError as e:
    print(f"OverflowError: {e}")

try:
    int(float('-inf'))
except OverflowError as e:
    print(f"OverflowError: {e}")

try:
    int(float('nan'))
except ValueError as e:
    print(f"ValueError: {e}")

# math.floor with inf/nan — must raise.
try:
    math.floor(float('inf'))
except OverflowError as e:
    print(f"OverflowError: {e}")

try:
    math.floor(float('nan'))
except ValueError as e:
    print(f"ValueError: {e}")

# math.ceil with inf/nan — must raise.
try:
    math.ceil(float('inf'))
except OverflowError as e:
    print(f"OverflowError: {e}")

try:
    math.ceil(float('nan'))
except ValueError as e:
    print(f"ValueError: {e}")

# math.trunc with inf/nan — must raise.
try:
    math.trunc(float('inf'))
except OverflowError as e:
    print(f"OverflowError: {e}")

try:
    math.trunc(float('nan'))
except ValueError as e:
    print(f"ValueError: {e}")
