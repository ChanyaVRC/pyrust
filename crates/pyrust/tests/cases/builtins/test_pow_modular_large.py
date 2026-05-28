# Parity fixture for issue #1697: modpow_i64 overflow with large moduli.
#
# Intermediate products in the binary-exponentiation loop can reach
# (modulus-1)^2 which overflows i64 when modulus > ~2^31.  The fix widens
# intermediates to i128.

# Original repro: modulus = 10^14 + 7 (just under 2^47), base after one
# squaring is ~10^28 — far beyond i64 range.
print(pow(10**9, 3, 10**14 + 7))

# Modulus just above the 2^31 boundary that triggers overflow.
print(pow(2**31, 4, 2**31 + 11))

# Very large modulus, still fits in i64.
print(pow(2, 62, 10**18 + 9))

# Large base and modulus.
print(pow(999999999999999, 2, 10**15 + 3))

# Negative base path.
print(pow(-10**9, 3, 10**14 + 7))

# Small values — no regression.
print(pow(3, 2, 7))
print(pow(0, 0, 5))
print(pow(1, 1000000, 7))

# Modular inverse path (exponent = -1).
print(pow(3, -1, 7))
print(pow(10**9 + 7, -1, 10**9 + 9))

# Typical competitive-programming case.
print(pow(10**9 + 7, 1000000007, 10**9 + 9))
