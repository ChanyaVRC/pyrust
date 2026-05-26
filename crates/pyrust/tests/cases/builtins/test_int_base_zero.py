# Parity fixture for int(string, 0) — auto-detect base from prefix.
# CPython 3.12 treats base=0 as "infer base from string prefix".

# Hex prefix
print(int("0xff", 0))   # 255
print(int("0xFF", 0))   # 255
print(int("0XFF", 0))   # 255

# Octal prefix
print(int("0o77", 0))   # 63
print(int("0O77", 0))   # 63

# Binary prefix
print(int("0b1010", 0))  # 10
print(int("0B1010", 0))  # 10

# Plain decimal (no prefix)
print(int("42", 0))      # 42
print(int("0", 0))       # 0
print(int("00", 0))      # 0

# Sign handling
print(int("-0b11", 0))   # -3
print(int("+0b11", 0))   # 3
print(int("-0xff", 0))   # -255

# Surrounding whitespace is stripped (CPython does this)
print(int("  0xFF  ", 0))  # 255

# BigInt: value exceeds i64::MAX — must promote to big int, not ValueError
print(int("0x8000000000000000", 0))   # 9223372036854775808
print(int("-0x8000000000000001", 0))  # -9223372036854775809

# Invalid: no prefix, not a valid decimal
try:
    int("FF", 0)
except ValueError as e:
    print(f"ValueError: {e}")

# Invalid: leading 0 followed by non-zero digit (Python 3 forbids legacy octal)
try:
    int("09", 0)
except ValueError as e:
    print(f"ValueError: {e}")

# Invalid: prefix present but no digits follow
try:
    int("0x", 0)
except ValueError as e:
    print(f"ValueError: {e}")

# No regression: non-zero explicit base still works
print(int("ff", 16))    # 255
print(int("77", 8))     # 63
print(int("1010", 2))   # 10
print(int("42", 10))    # 42
