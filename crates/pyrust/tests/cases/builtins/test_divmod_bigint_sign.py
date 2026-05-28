# Parity fixture: divmod() floor-division sign semantics for BigInt / i64-boundary values.
# Python uses floor division (round toward -inf), not truncation (round toward 0).
# Issue #1461: divmod(-2**63, 3) returned wrong quotient due to i64 overflow in fast path.

# i64::MIN dividend, positive divisor — the canonical repro from the issue.
print(divmod(-2**63, 3))   # (-3074457345618258603, 1)

# Small negative dividend (regression guard for the common case).
print(divmod(-10, 3))      # (-4, 2)

# Negative dividend, negative divisor (both signs negative → positive quotient via floor).
print(divmod(-2**63, -3))  # (3074457345618258602, -2)

# Positive dividend, negative divisor.
print(divmod(2**63, -3))   # (-3074457345618258603, -1)

# Larger-than-i64 dividend.
print(divmod(-(2**64), 7)) # (-2635249153387078803, 5)

# Zero remainder cases (no sign adjustment needed).
print(divmod(-9, 3))       # (-3, 0)
print(divmod(9, -3))       # (-3, 0)

# Positive dividend, positive divisor (no-adjustment baseline).
print(divmod(2**63, 3))    # (3074457345618258602, 2)

# Floor-division operator // and % should match.
print((-2**63) // 3)   # -3074457345618258603
print((-2**63) % 3)    # 1

# i64::MIN dividend, divisor = -1: quotient = 2**63 (overflows i64, must be BigInt).
print(divmod(-2**63, -1))  # (9223372036854775808, 0)
print((-2**63) // -1)      # 9223372036854775808
print((-2**63) % -1)       # 0
