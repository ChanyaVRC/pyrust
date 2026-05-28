# Parity tests for chr() with arguments that don't fit in a C int (#1584).
#
# CPython 3.12 converts chr()'s argument to a C int (int32_t, range
# [-2**31, 2**31-1]) before the Unicode range check.  Anything outside that
# range raises OverflowError("Python int too large to convert to C int").
# Values inside the C-int range but outside 0..0x110000 raise ValueError.

# Positive BigInt (> i64 max): too large to convert to C int.
try:
    chr(10**100)
except OverflowError as e:
    print(type(e).__name__, str(e))

# Negative BigInt (< i64 min): also too large to convert to C int.
try:
    chr(-10**100)
except OverflowError as e:
    print(type(e).__name__, str(e))

# 2**31: fits in i64 but exceeds C int max -> OverflowError (not ValueError).
try:
    chr(2**31)
except OverflowError as e:
    print(type(e).__name__, str(e))

# -(2**31 + 1): fits in i64 but below C int min -> OverflowError.
try:
    chr(-(2**31) - 1)
except OverflowError as e:
    print(type(e).__name__, str(e))

# 0x7fffffff: largest C int, outside Unicode range -> ValueError (not OverflowError).
try:
    chr(0x7fffffff)
except ValueError as e:
    print(type(e).__name__, str(e))

# Just out of Unicode range (fits in C int) -> ValueError.
try:
    chr(0x110000)
except ValueError as e:
    print(type(e).__name__, str(e))

# Last valid codepoint.
print(chr(0x10FFFF))
