# int(string, 0) — base=0 auto-detects numeric base from prefix.
#
# CPython 3.12 rules:
#   0x/0X prefix  → hexadecimal (base 16)
#   0b/0B prefix  → binary (base 2)
#   0o/0O prefix  → octal (base 8)
#   No prefix     → decimal (base 10)
#   Leading 0 without letter prefix is an error unless all digits are 0.

# Hex prefix
print(int("0xff", 0))       # 255
print(int("0xFF", 0))       # 255
print(int("0XFF", 0))       # 255 (uppercase X)
print(int("0x1F", 0))       # 31

# Binary prefix
print(int("0b1010", 0))     # 10
print(int("0B101", 0))      # 5

# Octal prefix
print(int("0o17", 0))       # 15
print(int("0O17", 0))       # 15 (uppercase O)

# Decimal (no prefix)
print(int("42", 0))         # 42
print(int("0", 0))          # 0
print(int("00", 0))         # 0
print(int("000", 0))        # 0

# Whitespace is stripped
print(int("  0x10  ", 0))   # 16
print(int("  42  ", 0))     # 42

# Signs
print(int("-0xff", 0))      # -255
print(int("+0xff", 0))      # 255
print(int("-0b101", 0))     # -5
print(int("-0o17", 0))      # -15
print(int("-42", 0))        # -42

# Error: empty digits after prefix
try:
    int("0x", 0)
except ValueError as e:
    print("ValueError:", e)

try:
    int("0b", 0)
except ValueError as e:
    print("ValueError:", e)

try:
    int("0o", 0)
except ValueError as e:
    print("ValueError:", e)

# Error: leading zero followed by non-zero digit (Python 3 forbids octal-style)
try:
    int("09", 0)
except ValueError as e:
    print("ValueError:", e)

try:
    int("01", 0)
except ValueError as e:
    print("ValueError:", e)

try:
    int("001", 0)
except ValueError as e:
    print("ValueError:", e)

# Error: non-string with explicit base
try:
    int(42, 0)
except TypeError as e:
    print("TypeError:", e)
