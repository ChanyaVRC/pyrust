# Parity fixture for round(int, negative_ndigits) (issue #1417).
# When ndigits < 0, round() must round to the nearest 10**(-ndigits)
# using banker's rounding (round half to even).

# Basic rounding to nearest 10
print(round(1234, -1))   # 1230
print(round(1235, -1))   # 1240 (5 is half, 3 is odd → round up)
print(round(1245, -1))   # 1240 (5 is half, 4 is even → round down)

# Rounding to nearest 100
print(round(1234, -2))   # 1200
print(round(1250, -2))   # 1200 (banker's: 12 is even → stay)
print(round(1350, -2))   # 1400 (banker's: 13 is odd → round up)
print(round(1567, -2))   # 1600

# Rounding to nearest 1000
print(round(1234, -3))   # 1000
print(round(2500, -3))   # 2000 (banker's: 2 is even → stay)
print(round(3500, -3))   # 4000 (banker's: 3 is odd → round up)

# Rounding to nearest 10000
print(round(1234, -4))   # 0
print(round(9999, -4))   # 10000

# Negative integers
print(round(-1234, -2))  # -1200
print(round(-1250, -2))  # -1200 (banker's)
print(round(-1350, -2))  # -1400 (banker's)

# Zero
print(round(0, -3))      # 0

# Bool treated as int (True=1, False=0)
print(round(True, -1))   # 0
print(round(False, -1))  # 0

# Positive ndigits and no ndigits: unchanged for int
print(round(1234))       # 1234
print(round(1234, 0))    # 1234
print(round(1234, 2))    # 1234

# Return type is int for all integer inputs with negative ndigits
print(type(round(1234, -2)))   # <class 'int'>
print(type(round(1234, -2)) is int)  # True

# Very large integer (BigInt territory)
big = 10 ** 30
print(round(big, -10))          # 10**30 (already exact multiple)
print(round(big + 1, -3))       # 10**30 (1 < 500 → round down)

# Result that overflows i64 (result is BigInt)
near_max = 9223372036854775000
print(round(near_max, -3))      # rounds near i64 boundary

# ndigits=None (same as absent) → return unchanged
print(round(1234, None))        # 1234
