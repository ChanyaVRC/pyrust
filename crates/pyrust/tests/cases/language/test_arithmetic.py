# Basic arithmetic, exponentiation, bitwise ops, integer literals

x = -7
y = 3
print("mod", x % y)
print("floordiv", x // y)

# Exponentiation
print("pow", 2 ** 10)
print("pow-float", 2 ** 0.5)
print("pow-neg-exp", 2 ** -1)

# Augmented assignment
x = 10
x += 3
print("aug-add", x)
x -= 2
print("aug-sub", x)
x *= 2
print("aug-mul", x)
x //= 3
print("aug-floordiv", x)
x %= 5
print("aug-mod", x)
x **= 3
print("aug-pow", x)

# Bitwise operations
print("bit-and", 0b1100 & 0b1010)
print("bit-or",  0b1100 | 0b1010)
print("bit-xor", 0b1100 ^ 0b1010)
print("bit-lsh", 1 << 4)
print("bit-rsh", 32 >> 2)
print("bit-not", ~7)

# Integer literals
print("hex", 0xFF)
print("oct", 0o77)
print("bin", 0b1010)
print("underscore", 1_000_000)

# Negative shift counts — Issue #90/#91
try:
    x = 1 << -1
except ValueError:
    print("lshift-neg", "ValueError")
try:
    x = 32 >> -1
except ValueError:
    print("rshift-neg", "ValueError")
