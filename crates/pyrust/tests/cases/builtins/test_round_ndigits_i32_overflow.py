# round(float, ndigits) — ndigits outside i32 range
#
# When ndigits is a Python int whose value does not fit in i32, pyrust was
# silently truncating via `as i32`, causing wrong results:
#   - 2**31 wrapped to i32::MIN (negative), entering the wrong branch.
#   - 2**32 wrapped to 0, computing round(x, 0) instead of returning x.
# The fix clamps ndigits to -(i32::MAX)..=i32::MAX before converting so that
# the existing is_infinite() overflow guards handle the out-of-range case.

# Large positive ndigits: float has insufficient precision to round at this
# many decimal places, so CPython returns it unchanged.
print(round(7.5, 2**31))       # 7.5
print(round(7.5, 2**32))       # 7.5
print(round(7.5, 2**63 - 1))   # 7.5  (i64::MAX)
print(round(1.5, 2**31))       # 1.5
print(round(-7.5, 2**31))      # -7.5

# Large negative ndigits: rounding to a magnitude far beyond f64 range gives 0.
print(round(7.5, -(2**31)))    # 0.0
print(round(7.5, -(2**31) - 1))  # 0.0
print(round(7.5, -(2**32)))    # 0.0

# Normal paths must not regress.
print(round(7.5, 2))           # 7.5
print(round(7.5, 0))           # 8.0  (banker's rounding)
print(round(2.5, 0))           # 2.0  (banker's rounding)
print(round(1234.5, -3))       # 1000.0
print(round(7.5, -1))          # 10.0

# round with no ndigits returns int.
print(round(1.5))              # 2
print(round(2.5))              # 2    (banker's rounding)

# Values just inside i32 range (the is_infinite() guard from PR #1511 applies).
print(round(7.5, 2147483647))  # 7.5  (i32::MAX; 10^MAX overflows → unchanged)
print(round(7.5, -2147483647)) # 0.0  (i32::MIN+1; factor overflows → 0)
