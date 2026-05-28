# Parity fixture for issue #1017: bytes ordering comparisons.
# CPython 3.12 compares bytes lexicographically by byte value.

# Basic ordering.
print(b"abc" < b"abd")   # True
print(b"abc" > b"abd")   # False
print(b"abc" <= b"abc")  # True
print(b"abc" >= b"abd")  # False

# Empty bytes is less than any non-empty bytes.
print(b"" < b"a")        # True
print(b"a" > b"")        # True

# Prefix comparison: longer beats shorter when prefix matches.
print(b"abc" < b"ab")    # False
print(b"ab" < b"abc")    # True

# Equal bytes: le/ge both True, lt/gt both False.
print(b"abc" <= b"abc")  # True
print(b"abc" >= b"abc")  # True
print(b"abc" < b"abc")   # False
print(b"abc" > b"abc")   # False

# Byte value ordering (unsigned: 0x80 > 0x7f).
print(b"\x80" > b"\x7f")  # True
print(b"\x00" < b"\x01")  # True

# Equality still works (regression guard).
print(b"abc" == b"abc")  # True
print(b"abc" != b"abd")  # True

# Comparing bytes with non-bytes raises TypeError.
try:
    print(b"x" < 1)
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    print(b"x" > "x")
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))
