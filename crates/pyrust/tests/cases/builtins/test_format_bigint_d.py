# Parity fixture for issue #1269: str.format() {:d} with BigInt values.
#
# Targeted regression test for the {:d} decimal format code on integers
# beyond i64 range. The original bug was ValueError for BigInt with {:d}.
# Comprehensive coverage of all integer format codes is in test_format_bigint.py.

# Basic decimal {:d}
print("{:d}".format(10**100))
print("{:d}".format(-10**100))
print("{:d}".format(10**50))
print("{:d}".format(-(10**50)))

# Boundary: first BigInt (2**63 exceeds i64::MAX)
print("{:d}".format(2**63))
print("{:d}".format(-(2**63 + 1)))

# Width and alignment
print("{:20d}".format(10**30))
print("{:<20d}".format(10**5))
print("{:^30d}".format(10**10))

# Sign flags
print("{:+d}".format(10**30))
print("{:+d}".format(-(10**30)))
print("{: d}".format(10**30))

# Thousands separator
print("{:,d}".format(10**20))
print("{:_d}".format(10**20))

# Zero-pad
print("{:050d}".format(10**30))

# Regression: small i64 integers still work
print("{:d}".format(42))
print("{:d}".format(-42))
print("{:d}".format(0))
print("{:+d}".format(0))
