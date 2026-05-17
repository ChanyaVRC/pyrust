# Parity fixture for int == float comparisons beyond the 53-bit mantissa.
# CPython issue: 2**53+1 == float(2**53+1) must be False because the float
# rounds to 2**53 (nearest-even), so the two sides are not equal.

# Exact boundary: 2**53 is the largest integer exactly representable as f64.
print(2**53 == float(2**53))           # True
print(2**53 - 1 == float(2**53 - 1))  # True

# Beyond 53-bit precision: the float rounds away, so not equal.
print(2**53 + 1 == float(2**53 + 1))  # False — float rounds to 2**53
print(2**53 + 3 == float(2**53 + 3))  # False — float rounds to 2**53 + 2

# 2**53 + 2 is even so representable exactly.
print(2**53 + 2 == float(2**53 + 2))  # True

# Comparing int to the wrong float value (both sides in range).
print(2**53 + 1 == float(2**53))      # False

# Negative mirror.
print(-(2**53 + 1) == -float(2**53))  # False

# Small integers: exact at all precision levels.
print(1 == 1.0)                        # True
print(0 == 0.0)                        # True
print(-1 == -1.0)                      # True
print(1 == 1.5)                        # False

# Symmetry: float on left side.
print(1.0 == 1)                        # True
print(float(2**53) == 2**53 + 1)      # False

# Not-equal operator.
print(2**53 + 1 != float(2**53 + 1))  # True
print(1 != 1.0)                        # False

# Non-finite floats are never equal to any integer.
print(1 == float('inf'))               # False
print(0 == float('nan'))               # False
print(0 == float('-inf'))              # False
print(float('inf') == 1)               # False
print(float('nan') == 0)               # False

# 10**16 and 10**18: both happen to be exactly representable as f64.
print(10**16 == float(10**16))         # True
print(10**18 == float(10**18))         # True
