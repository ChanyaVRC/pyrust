# round(float, large_ndigits) — guards against f64 overflow in the factor.
# When ndigits is so large that 10**ndigits (or v * 10**ndigits) overflows f64,
# CPython returns the float unchanged.  When ndigits is so negative that
# 10**(-ndigits) overflows f64, CPython returns signed zero (for finite values).

# Reported repro (issue #1485): these returned NaN instead of the float.
print(round(7.5, 10**6))   # 7.5
print(round(7.5, 10**9))   # 7.5
print(round(-7.5, 10**6))  # -7.5

# Boundary around 10^308 — factor itself is finite but v*factor overflows.
print(round(7.5, 308))     # 7.5 (CPython: v*factor overflows, return v)
print(round(7.5, 309))     # 7.5 (CPython: factor overflows to inf, return v)

# Normal rounding must still work.
print(round(7.5, 2))       # 7.5
print(round(1234.5, -3))   # 1000.0

# Large-negative ndigits: finite values collapse to signed zero.
print(round(7.5, -309))    # 0.0
print(round(-7.5, -309))   # -0.0

# Non-finite inputs pass through unchanged regardless of ndigits.
print(round(float('inf'), 309))    # inf
print(round(float('-inf'), 309))   # -inf
print(round(float('nan'), 309))    # nan
print(round(float('inf'), -309))   # inf
print(round(float('-inf'), -309))  # -inf
print(round(float('nan'), -309))   # nan
