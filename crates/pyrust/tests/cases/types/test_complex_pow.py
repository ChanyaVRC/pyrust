# Parity fixture for complex exponentiation (issue #623).
# All cases must produce byte-for-byte identical output under CPython 3.12
# and pyrust.

# Real base, complex exponent
print(repr(2 ** (1+0j)))         # (2+0j)
print(repr(2 ** (0+1j)))         # exp(i*ln(2))

# Complex base, integer exponent (uses repeated squaring for exact result)
print(repr((1+1j) ** 2))         # 2j (exact, no floating-point rounding)
print(repr((1+1j) ** 0))         # (1+0j)
print(repr((1+1j) ** 100))       # (-1125899906842624+0j)
print(repr((2+3j) ** 2))         # (-5+12j)

# Complex base, float exponent
print(repr((2+0j) ** 0.5))       # (1.4142135623730951+0j)
print(repr((2+3j) ** 0.5))

# Complex base, complex exponent
print(repr((1+1j) ** (1+1j)))    # (0.2739572538301211+0.5837007587586147j)
print(repr((2+3j) ** (1+2j)))

# Negative real base as complex
print(repr((-1+0j) ** 0.5))      # (6.123233995736766e-17+1j)

# Zero base
print(repr(0j ** 0))             # (1+0j) -- z**0 = 1 for any z
print(repr(0j ** 1))             # 0j
print(repr(0j ** 2))             # 0j
print(repr((0+0j) ** (2+0j)))    # 0j

# Regression: non-complex paths must be unaffected
print(repr(2 ** 10))             # 1024
print(repr(2.5 ** 2))            # 6.25
print(repr((-2) ** 3))           # -8

# Zero-base error cases
try:
    _ = 0j ** (-1)
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

try:
    _ = 0j ** (1+1j)
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

try:
    _ = 0j ** (0+1j)
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")

try:
    _ = 0j ** (-0.5)
except ZeroDivisionError as e:
    print(f"ZeroDivisionError: {e}")
