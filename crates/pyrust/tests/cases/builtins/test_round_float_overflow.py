# Parity fixture for issue #1514:
# round(large_float, very_negative_ndigits) should raise OverflowError when
# the rounded result cannot be represented as a finite float.
#
# Root cause: powi(n) (repeated squarings) accumulates ULP error at high
# exponents and gives a different value than libm pow() used by CPython.
# Fix: use powf(n as f64) which matches CPython's pow() precision.

def try_round(v, n):
    try:
        result = round(v, n)
        print(repr(result))
    except OverflowError as e:
        print(f"OverflowError: {e}")

# round(1.5e308, -308): 1.5e308 / 1e308 = 1.5, rounds to 2, 2 * 1e308 overflows
try_round(1.5e308, -308)

# round(-1.5e308, -308): same as above but negative
try_round(-1.5e308, -308)

# round(1e308, -308): 1e308 / 1e308 = 1.0, rounds to 1, 1 * 1e308 = 1e308 (ok)
try_round(1e308, -308)

# round(9.9e307, -308): 9.9e307 / 1e308 = 0.99, rounds to 1, 1 * 1e308 = 1e308 (ok)
try_round(9.9e307, -308)

# round(1e308, -400): factor = 10^400 overflows, finite value rounds to 0.0
try_round(1e308, -400)

# round(1e308, -309): factor = 10^309 overflows, finite value rounds to 0.0
try_round(1e308, -309)

# Non-finite floats pass through (no error): CPython propagates inf/nan as-is
try_round(float('inf'), -100)
try_round(float('nan'), -100)

# round(1.5e308, 0): unaffected (uses n >= 0 code path)
try_round(1.5e308, 0)

# Large negative value that rounds precisely to a non-overflowing result
try_round(-9.9e307, -308)
