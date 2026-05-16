# Parity fixture for hash() on BigInt values (issue #504).
#
# CPython reduces all integers modulo 2^61-1 (Py_HASH_MODULUS) and remaps
# the result -1 to -2 (the C-level tp_hash error sentinel).
#
# Values that fit in i64 but exceed the Mersenne prime (2^61-1) are reduced
# the same way; the tests below cover both the BigInt-in-Rust-terms path
# (|n| > i64::MAX) and the large-i64 path (|n| <= i64::MAX but >= 2^61-1).

M = (1 << 61) - 1

# The Mersenne prime itself reduces to 0 (M mod M == 0).
print(hash(M))            # 0

# One step past the modulus.
print(hash(M + 1))        # 1

# A large positive BigInt (well above i64::MAX).
print(hash(2**100))       # 549755813888

# Negative counterpart — reduction preserves sign.
print(hash(-(2**100)))    # -549755813888

# Smallest value that overflows i64 in pyrust's representation.
print(hash(2**63))        # 4

# Large i64 values in the range (M, i64::MAX].
print(hash(2**62))        # 2
print(hash(2**63 - 1))   # 3

# Negative values that trigger the -1 -> -2 sentinel remap.
print(hash(-1))           # -2
print(hash(-(M + 1)))    # -2  (reduces to -1, then remapped)

# Negative Mersenne prime reduces to 0.
print(hash(-M))           # 0

# BigInt keys work in dict and set.
d = {2**100: "ok"}
print(d[2**100])          # ok

s = {2**100, 2**200, 2**100}
print(len(s))             # 2
