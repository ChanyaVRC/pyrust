# Parity fixture: hex(), oct(), bin() with BigInt arguments (values > i64::MAX).
# CPython 3.12 reference: https://docs.python.org/3.12/library/functions.html#hex
# Regression: pyrust previously raised OverflowError for BigInt inputs (#1226).

# Positive BigInts
print(hex(2**64))    # 0x10000000000000000
print(hex(2**128))   # 0x100000000000000000000000000000000
print(bin(2**100))   # 0b + 101 binary digits
print(oct(2**64))    # 0o2000000000000000000000

# Negative BigInts
print(hex(-(2**64)))   # -0x10000000000000000
print(oct(-(2**64)))   # -0o2000000000000000000000
print(bin(-(2**64)))   # -0b + 65 binary digits

# Boundary: i64::MAX (small) and i64::MAX + 1 (BigInt)
print(hex(9223372036854775807))   # 0x7fffffffffffffff
print(hex(9223372036854775808))   # 0x8000000000000000
print(hex(-9223372036854775808))  # -0x8000000000000000
print(hex(-9223372036854775809))  # -0x8000000000000001

# oct/bin at i64::MAX+1
print(oct(9223372036854775808))   # 0o1000000000000000000000
print(bin(9223372036854775808))   # 0b1000000000000000000000000000000000000000000000000000000000000000

# Small ints still work correctly
print(hex(0))     # 0x0
print(hex(255))   # 0xff
print(hex(-1))    # -0x1
print(oct(0))     # 0o0
print(bin(0))     # 0b0

# __index__ returning BigInt should also work
class BigIndexed:
    def __index__(self):
        return 2**65

print(hex(BigIndexed()))   # 0x20000000000000000
print(oct(BigIndexed()))   # 0o4000000000000000000000
print(bin(BigIndexed()))   # 0b100000000000000000000000000000000000000000000000000000000000000000
