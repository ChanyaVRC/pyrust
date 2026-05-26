# Parity fixture for issue #1269: str.format() {:d}/{:b}/{:x}/{:o} with BigInt.
#
# BigInt values (beyond i64 range) must format correctly with all integer
# format codes.  Previously {:d} raised ValueError for BigInt.

n = 10**30

# Basic decimal
print("{:d}".format(n))
print("{:d}".format(10**100))
print("{:d}".format(-(10**30)))

# Hex, binary, octal
print("{:x}".format(n))
print("{:X}".format(n))
print("{:b}".format(n))
print("{:o}".format(n))

# Alt prefixes
print("{:#x}".format(n))
print("{:#X}".format(n))
print("{:#b}".format(n))
print("{:#o}".format(n))

# Negative values with prefixes
print("{:#x}".format(-(10**20)))
print("{:#b}".format(-(10**20)))

# Sign flags
print("{:+d}".format(n))
print("{: d}".format(n))
print("{:+d}".format(-(10**20)))

# Grouping separators
print("{:,d}".format(10**20))
print("{:_d}".format(10**20))
print("{:_x}".format(10**20))
print("{:_b}".format(10**20))

# Width / alignment
print("{:>50d}".format(n))
print("{:<50d}".format(n))
print("{:^50d}".format(n))
print("{:050d}".format(n))
print("{:*>50d}".format(n))

# 'n' type code (locale-neutral decimal, same as 'd' in pyrust)
print("{:n}".format(n))

# 'c': BigInt is always beyond C long range → OverflowError with CPython message
try:
    print("{:c}".format(10**30))
except OverflowError as e:
    print(type(e).__name__ + ": " + str(e))

try:
    print("{:c}".format(-(10**30)))
except OverflowError as e:
    print(type(e).__name__ + ": " + str(e))

# Regression: small integers still work
print("{:d}".format(42))
print("{:d}".format(-42))
print("{:b}".format(42))
print("{:x}".format(255))
print("{:o}".format(8))
