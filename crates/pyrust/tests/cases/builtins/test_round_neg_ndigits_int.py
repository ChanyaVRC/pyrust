# Parity fixture for round(int, negative_ndigits) (issue #1026).
# Verifies the exact repro cases from the bug report and acceptance criteria.

# Original repro from issue #1026
print(round(1234, -2))   # 1200
print(round(1250, -2))   # 1200 (banker's rounding: 12 is even)
print(round(1350, -2))   # 1400 (banker's rounding: 13 is odd)
print(round(12345, -3))  # 12000

# Negative values
print(round(-1234, -2))  # -1200
print(round(-1250, -2))  # -1200 (banker's rounding)

# Bool is treated as int (True=1, False=0)
print(round(True, -1))   # 0
print(round(False, -1))  # 0

# Positive ndigits and absent ndigits leave int unchanged
print(round(1234, 2))    # 1234
print(round(1234))       # 1234

# BigInt: round(10**100, -5) is already an exact multiple of 10**5
print(round(10**100, -5) == 10**100)  # True
# round(10**100 + 1, -5) rounds down to 10**100
print(round(10**100 + 1, -5) == 10**100)  # True
