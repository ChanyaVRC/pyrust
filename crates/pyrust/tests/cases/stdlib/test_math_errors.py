"""
Parity tests for math module domain and range error handling.

CPython raises ValueError("math domain error") when a finite input maps to NaN
or -inf (undefined domain), and OverflowError("math range error") when a finite
input maps to +inf (range overflow).  Non-finite inputs propagate without error.
"""

import math


def check_valueerror(fn, *args):
    try:
        result = fn(*args)
        print(f"MISSED: {fn.__name__}{args!r} returned {result!r}, expected ValueError")
    except ValueError as e:
        print(f"ValueError: {e}")
    except Exception as e:
        print(f"WRONG: {type(e).__name__}: {e}")


def check_overflowerror(fn, *args):
    try:
        result = fn(*args)
        print(f"MISSED: {fn.__name__}{args!r} returned {result!r}, expected OverflowError")
    except OverflowError as e:
        print(f"OverflowError: {e}")
    except Exception as e:
        print(f"WRONG: {type(e).__name__}: {e}")


# -- sqrt --
check_valueerror(math.sqrt, -1)
check_valueerror(math.sqrt, -0.5)
print(math.sqrt(4))          # 2.0
print(math.sqrt(0))          # 0.0
print(math.sqrt(float('inf')))   # inf  (not an error)
print(math.sqrt(float('nan')))   # nan  (not an error)

# -- log (one-arg) --
check_valueerror(math.log, -1)
check_valueerror(math.log, 0)
check_valueerror(math.log, -0.5)
print(math.log(1))           # 0.0
print(math.log(float('inf')))    # inf  (not an error)
print(math.log(float('nan')))    # nan  (not an error)

# -- log (two-arg) --
check_valueerror(math.log, -1, 2)
check_valueerror(math.log, 0, 2)
check_valueerror(math.log, 1, -1)
print(math.log(8, 2))        # 3.0

# -- log2 --
check_valueerror(math.log2, -1)
check_valueerror(math.log2, 0)
print(math.log2(8))          # 3.0
print(math.log2(float('inf')))   # inf  (not an error)

# -- log10 --
check_valueerror(math.log10, -1)
check_valueerror(math.log10, 0)
print(math.log10(100))       # 2.0
print(math.log10(float('inf')))  # inf  (not an error)

# -- asin --
check_valueerror(math.asin, 2)
check_valueerror(math.asin, -2)
print(math.asin(0))          # 0.0
print(math.asin(1))          # ~1.5707...
print(math.asin(float('nan')))   # nan  (not an error)

# -- acos --
check_valueerror(math.acos, 2)
check_valueerror(math.acos, -2)
print(math.acos(1))          # 0.0
print(math.acos(float('nan')))   # nan  (not an error)

# -- exp --
check_overflowerror(math.exp, 710)
print(math.exp(0))           # 1.0
print(math.exp(1))           # ~2.718...
print(math.exp(float('-inf')))   # 0.0   (not an error)
print(math.exp(float('inf')))    # inf   (not an error — input is inf)

# -- pow --
check_valueerror(math.pow, -1, 0.5)
check_valueerror(math.pow, 0, -1)
check_overflowerror(math.pow, 1e300, 2)
print(math.pow(2, 3))        # 8.0
print(math.pow(float('inf'), 2))   # inf  (not an error)
print(math.pow(float('-inf'), 2))  # inf  (not an error)
