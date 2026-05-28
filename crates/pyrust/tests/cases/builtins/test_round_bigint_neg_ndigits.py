# Parity fixture for round(bigint_x, large_negative_ndigits) hang fix (#1566).
#
# When ndigits is a very large negative BigInt, the old code clamped neg_n to
# i32::MAX (~2 billion) and then tried to compute 10^2_147_483_647, hanging.
# The fix adds an early-exit: if neg_n > decimal_digits(|x|), return 0.
#
# NOTE: Cases like round(10**30, -(10**30)) also hang CPython (same bug),
# so they are omitted here.  The parity fixture covers boundary values that
# both runtimes handle without hanging (i.e., small concrete integer ndigits).

# 10**30 has 31 decimal digits.

# neg_n at the boundary: 10**30 rounds to 0 (< half = 5*10^30) at neg_n=31.
print(round(10**30, -31))           # 0
# neg_n = 30: already a multiple of 10^30, stays unchanged.
print(round(10**30, -30))           # 1000000000000000000000000000000
# neg_n = 29: last digit stripped, still a multiple of 10^29.
print(round(10**30, -29))           # 1000000000000000000000000000000
# neg_n > decimal_digits: early-exit returns 0.
print(round(10**30, -32))           # 0
print(round(-10**30, -32))          # 0
print(round(-10**30, -31))          # 0
print(round(-10**30, -30))          # -1000000000000000000000000000000

# 9*10^30 has 31 decimal digits; at neg_n=31 it rounds UP to 10^31.
print(round(9 * 10**30, -31))       # 10000000000000000000000000000000
# At neg_n=32, early-exit -> 0.
print(round(9 * 10**30, -32))       # 0

# Banker's rounding tie at boundary: 5*10^30 / 10^31 ties, q=0 is even -> 0.
print(round(5 * 10**30, -31))       # 0
# 15*10^30 / 10^31 = 1.5, ties, q=1 is odd -> round up to 2*10^31.
print(round(15 * 10**30, -31))      # 20000000000000000000000000000000

# Normal BigInt rounding (regression guard).
print(round(10**30, -1))            # 1000000000000000000000000000000
print(round(10**30 + 1, -1))        # 1000000000000000000000000000000
print(round(10**30 + 5, -1))        # 1000000000000000000000000000000 (tie, q even)
