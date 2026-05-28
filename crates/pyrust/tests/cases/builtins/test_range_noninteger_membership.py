# Parity fixture: float and complex membership in range()
# CPython 3.12 uses an integer-value fast path for float/complex instead of
# always returning False. See issue #1651.

# Float: integer-valued floats that are in range
print(1.0 in range(5))       # True
print(0.0 in range(5))       # True
print(4.0 in range(5))       # True

# Float: integer-valued floats out of range
print(5.0 in range(5))       # False
print(-1.0 in range(5))      # False

# Float: non-integer floats
print(1.5 in range(5))       # False
print(0.5 in range(5))       # False

# Float: special values
print(float('nan') in range(5))   # False
print(float('inf') in range(5))   # False
print(float('-inf') in range(5))  # False

# Float with step
print(2.0 in range(0, 10, 2))    # True
print(3.0 in range(0, 10, 2))    # False
print(8.0 in range(10, 0, -2))   # True
print(9.0 in range(10, 0, -2))   # False

# Complex: zero imaginary part + integer real part
print((1+0j) in range(5))    # True
print((0+0j) in range(5))    # True
print((4+0j) in range(5))    # True
print((5+0j) in range(5))    # False

# Complex: non-zero imaginary part
print((1+1j) in range(5))    # False
print((0+1j) in range(5))    # False

# Complex: non-integer real part
print((1.5+0j) in range(5))  # False

# String: never in range
print('x' in range(5))       # False
print('1' in range(5))       # False

# Boundary: float(i64::MAX) rounds to the same f64 as float(2**63).
# int(float(i64::MAX)) == 2**63, which is NOT in any i64-bounded range.
# Guard must use explicit bounds check, not the round-trip (f as i64) as f64 == f,
# because that round-trip cannot distinguish i64::MAX from i64::MAX+1.
i64max = 9223372036854775807
print(float(i64max) in range(i64max, i64max - 5, -1))      # False
print(float(i64max - 1024) in range(i64max - 1024, i64max - 1020, 1))  # True
