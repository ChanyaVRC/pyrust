"""
Parity fixture: int() and math.trunc/floor/ceil boundary at i64::MAX.

i64::MAX = 2**63 - 1 is not exactly representable as f64; it rounds up
to 2**63 = 9223372036854775808.0.  Any float value >= 2**63 must produce a
Python bigint, not be silently truncated to i64::MAX via Rust's saturating cast.

The lower bound (i64::MIN = -(2**63)) IS exactly representable as f64, so
values == i64::MIN as f64 can still fit in i64 and must not be promoted.
"""
import math

# --- int() from float ---

# float(2**63 - 1) == 2**63 (i64::MAX rounds up to 2^63 as f64)
# CPython 3.12: int(9223372036854775808.0) = 9223372036854775808 (bigint)
print(int(float(2**63 - 1)))

# A clearly-large float also becomes bigint
print(int(9.2e18))

# Literal 2**63 as float — same as float(2**63 - 1)
print(int(9223372036854775808.0))

# Just below the boundary: a float that truncates cleanly to a value < 2**63
# 9.2e18 < 2**63 is only true for values whose exact integer is < 2**63;
# confirm a sub-boundary value round-trips correctly
print(int(9000000000000000000.0))

# i64::MIN = -(2**63) is exactly representable; result must still fit in i64
print(int(float(-(2**63))))

# Normal float-to-int conversions must still work
print(int(1.9))
print(int(-1.9))
print(int(0.0))
print(int(-0.0))

# --- math.trunc() ---

print(math.trunc(float(2**63 - 1)))
print(math.trunc(9223372036854775808.0))
print(math.trunc(float(-(2**63))))
print(math.trunc(1.9))
print(math.trunc(-1.9))

# --- math.floor() ---

print(math.floor(float(2**63 - 1)))
print(math.floor(9223372036854775808.0))
print(math.floor(float(-(2**63))))

# --- math.ceil() ---

print(math.ceil(float(2**63 - 1)))
print(math.ceil(9223372036854775808.0))
print(math.ceil(float(-(2**63))))
