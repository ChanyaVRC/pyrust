# Parity fixture for BinOpImm: small integer immediates inlined in the
# instruction word, bypassing the constant pool.

# Loop increment (+= 1): the most common BinOpImm pattern.
x = 0
for _ in range(5):
    x += 1
print(x)  # 5

# Multiply by small integer.
y = 7
y *= 2
print(y)  # 14

# Comparison with zero (> 0, < 0).
z = 100
print(z > 0)   # True
print(z < 0)   # False

# Subtraction by 1.
a = 10
for _ in range(3):
    a -= 1
print(a)  # 7

# BinOpImm covers the full i16 range: test with boundary values.
big = 32767
big += 1
print(big)  # 32768

small = -32768
small -= 1
print(small)  # -32769

# Values just outside i16 range stay in the constant pool (BinOpConst).
large = 32768
large += 32768
print(large)  # 65536

# Augmented multiply / floor-divide / mod with small immediate.
m = 9
m //= 3
print(m)  # 3

n = 10
n %= 3
print(n)  # 1

# Comparison operators with non-zero immediate.
p = 5
print(p >= 5)   # True
print(p != 3)   # True
print(p == 5)   # True
