# Test BigInt + complex arithmetic (issue #617).
# CPython 3.12 coerces BigInt to float before performing complex arithmetic;
# pyrust was raising TypeError because as_complex_pair lacked a BigInt arm.

# Addition: BigInt on left
print(10**20 + 0j)          # (1e+20+0j)
print(10**20 + 1j)          # (1e+20+1j)
print(10**20 + (2+3j))      # (1e+20+3j)

# Addition: BigInt on right
print(0j + 10**20)          # (1e+20+0j)
print(1j + 10**20)          # (1e+20+1j)
print((1+2j) + 10**20)      # (1e+20+2j)

# Subtraction
print(10**20 - (2+3j))      # (1e+20-3j)
print((2+3j) - 10**20)      # (-1e+20+3j)

# Multiplication
print(10**20 * 1j)           # 1e+20j
print(10**20 * (1+1j))       # (1e+20+1e+20j)
print((2+3j) * 10**20)       # (2e+20+3e+20j)

# Overflow: BigInt too large for f64 -> OverflowError (not TypeError)
try:
    x = 10**400 + 0j
    print(x)
except OverflowError as e:
    print(f"OverflowError: {e}")

try:
    x = 0j + 10**400
    print(x)
except OverflowError as e:
    print(f"OverflowError: {e}")

try:
    x = 10**400 * 1j
    print(x)
except OverflowError as e:
    print(f"OverflowError: {e}")

# Regression: int + complex, float + complex, bool + complex still work
print(1 + 2j)           # (1+2j)
print(1.5 + 2j)         # (1.5+2j)
print(True + 2j)        # (1+2j)
print(2j - False)       # 2j
