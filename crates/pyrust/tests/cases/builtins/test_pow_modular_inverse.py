# parity fixture for pow(base, negative_exp, mod) — modular inverse (issue #1664)

# Basic modular inverse
print(pow(3, -1, 7))    # 5  (3*5 = 15 = 2*7+1)
print(pow(2, -3, 9))    # 8  (2^3=8, modinv(8,9)=8 since 8*8=64=7*9+1)
print(pow(3, -5, 7))    # 3

# mod 1: any integer mod 1 is 0
print(pow(3, -1, 1))    # 0
print(pow(3, -3, 1))    # 0

# Negative modulus: result has same sign as modulus
print(pow(3, -1, -7))   # -2
print(pow(3, -1, -1))   # 0

# Negative base with negative exponent
print(pow(-3, -1, 7))   # 2  (modinv(-3 mod 7 = 4, 7) = 2)

# Positive exponent still works (no regression)
print(pow(3, 2, 7))     # 2
print(pow(2, 10, 1000)) # 24

# ValueError: base not invertible (gcd(base, mod) != 1)
try:
    pow(2, -1, 4)
except ValueError as e:
    print(e)

# ValueError: gcd(0, mod) != 1 for any mod > 1
try:
    pow(0, -1, 5)
except ValueError as e:
    print(e)

# ValueError: mod == 0 (pre-existing, negative exp case)
try:
    pow(3, -1, 0)
except ValueError as e:
    print(e)

# BigInt arguments: 10**20 forces BigInt path
big = 10 ** 20
print(pow(big + 3, -1, 7))        # modinv((10^20+3) mod 7, 7)
print(pow(3, -(big + 1), 7))      # modinv(3^(10^20+1) mod 7, 7)
