# Parity fixture for round(float, ndigits) IEEE 754 precision (issues #1361, #1553).
#
# The naive multiply-round-divide approach loses precision when the float's exact
# rational value differs from its float64 representation of the scaled value.
# CPython uses exact decimal string conversion internally to avoid this.

# Positive ndigits: values near IEEE 754 midpoints
# 2.675 exact value is slightly below 2.675 → rounds down to 2.67, not 2.68
print(round(2.675, 2))    # 2.67
print(round(2.685, 2))    # 2.69
print(round(1.005, 2))    # 1.0
print(round(1.23456, 3))  # 1.235

# Negative ndigits: issue #1553
# 5e307 exact value is slightly above 5*10^307 → true quotient > 0.5 → rounds up
print(round(5e307, -308))   # 1e+308
print(round(-5e307, -308))  # -1e+308

# Half-even rounding still applies correctly
print(round(2.5))          # 2 (half-even: 2 is even)
print(round(3.5))          # 4 (half-even: 4 is even)
print(round(-2.5))         # -2
print(round(-3.5))         # -4
print(round(2.5, 0))       # 2.0
print(round(-2.5, 0))      # -2.0

# Negative values near midpoints
print(round(-2.675, 2))    # -2.67
print(round(-2.685, 2))    # -2.69

# Negative ndigits: basic cases
print(round(15.0, -1))    # 20.0 (half-even: 2 is even)
print(round(25.0, -1))    # 20.0 (half-even: 2 is even)
print(round(35.0, -1))    # 40.0 (half-even: 4 is even)
print(round(1234567.0, -3))  # 1235000.0

# Overflow: rounded result exceeds f64 range
try:
    print(round(1.5e308, -308))
except OverflowError as e:
    print(f"OverflowError: {e}")

# Large ndigits: float has insufficient precision → return unchanged
print(round(1.5, 20))      # 1.5

# Special values pass through
print(round(float('inf'), 2))   # inf
print(round(float('nan'), 2))   # nan
print(round(float('inf'), -2))  # inf
print(round(float('nan'), -2))  # nan

# Very small negative ndigits → 0.0
print(round(1.5, -1000))   # 0.0
