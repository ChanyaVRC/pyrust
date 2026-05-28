# Parity tests for chr() with BigInt arguments that don't fit in C int (#1584).
#
# CPython 3.12 raises OverflowError("Python int too large to convert to C int")
# for any integer argument that doesn't fit in a C int, regardless of whether
# it's positive or negative.  Values that fit in i64 but exceed the Unicode
# range (0x110000) raise ValueError instead.

# Positive BigInt: too large to convert to C int.
try:
    chr(10**100)
except OverflowError as e:
    print(type(e).__name__, str(e))

# Negative BigInt: also too large to convert to C int.
try:
    chr(-10**100)
except OverflowError as e:
    print(type(e).__name__, str(e))

# Just out of range (fits in i64, but exceeds Unicode range) -> ValueError.
try:
    chr(0x110000)
except ValueError as e:
    print(type(e).__name__, str(e))

# Last valid codepoint.
print(chr(0x10FFFF))
