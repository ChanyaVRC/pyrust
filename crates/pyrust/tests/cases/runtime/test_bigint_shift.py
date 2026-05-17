# Left shift promoting to BigInt when the result overflows i64.
print(1 << 63)    # 9223372036854775808
print(1 << 64)    # 18446744073709551616
print(1 << 100)   # 1267650600228229401496703205376
print((-1) << 63) # -9223372036854775808
print(type(1 << 100).__name__)  # int

# Non-constant operands (runtime dispatch path).
a = 1
n = 100
print(a << n)     # 1267650600228229401496703205376

# Right shift of BigInt.
x = 1 << 100
print(x >> 50)    # 1125899906842624
print(x >> 100)   # 1
print(x >> 101)   # 0

# Large right shift saturates to sign bit.
print(-1 >> 64)   # -1
print(0 >> 64)    # 0

# Negative shift count raises ValueError.
try:
    _ = 1 << -1
    print('no error')
except ValueError:
    print('ValueError')  # ValueError

try:
    _ = 1 >> -1
    print('no error')
except ValueError:
    print('ValueError')  # ValueError
