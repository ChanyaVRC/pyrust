# Parity fixture for round() with BigInt ndigits (#1547).
# CPython accepts any integer type for ndigits; very large positive ndigits
# leave the float unchanged (float precision is exhausted), and very large
# negative ndigits round to 0.0.

# Very large positive ndigits — float is returned unchanged.
print(round(7.5, 10**30))        # 7.5
print(round(1.0, 10**100))       # 1.0
print(round(3.14159, 10**20))    # 3.14159

# Very large negative ndigits — finite floats round to 0.0.
print(round(7.5, -(10**30)))     # 0.0
print(round(0.0, -(10**30)))     # 0.0
print(round(1234567.89, -(10**20)))  # 0.0

# Normal small ndigits still work correctly (regression guard).
print(round(7.5, 2))             # 7.5  (banker's rounding: 7.5 stays at 7.5 with n=2)
print(round(2.5))                # 2    (banker's rounding rounds to even)
print(round(1.5))                # 2
