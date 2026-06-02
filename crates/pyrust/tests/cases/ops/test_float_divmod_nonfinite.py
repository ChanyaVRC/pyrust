# Parity fixture for issue #2025:
# Float floor-division (//) and divmod use CPython's fmod-based float_divmod,
# so infinities and signed zeros propagate correctly and
# divmod(a, b) == (a // b, a % b) holds for floats.

inf = float('inf')

# Infinite dividend → nan quotient (not inf).
print(inf // 1)
print(inf // 2.0)
print(divmod(inf, 1))

# Infinite divisor → finite quotient/remainder.
print(divmod(5.0, inf))
print(divmod(-5.0, inf))

# The % operator (already correct) must agree with divmod's remainder.
print(5.0 % inf)
print(-5.0 % inf)

# divmod(a, b) == (a // b, a % b) consistency across signs and signed zero.
cases = [
    (7.5, 2.0), (-7.5, 2.0), (7.5, -2.0), (-7.5, -2.0),
    (5.0, inf), (-5.0, inf), (5.0, -inf), (-5.0, -inf),
    (0.0, 3.0), (-0.0, 3.0), (0.0, -3.0), (-0.0, -3.0),
    (1.0, 0.5), (-1.0, 0.5),
]
for a, b in cases:
    q, m = divmod(a, b)
    print(repr(q), repr(m), repr(a // b), repr(a % b))
