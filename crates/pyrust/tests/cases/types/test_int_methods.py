# bit_length: number of bits required to represent abs(n) in binary
print((0).bit_length())    # 0
print((1).bit_length())    # 1
print((5).bit_length())    # 3
print((255).bit_length())  # 8
print((-1).bit_length())   # 1 — uses abs value
print((-255).bit_length()) # 8 — uses abs value

# bit_length on BigInt (> i64::MAX)
print((2**100).bit_length())    # 101
print((2**100 - 1).bit_length()) # 100

# bit_count: number of 1-bits in abs(n)
print((0).bit_count())     # 0
print((5).bit_count())     # 2 — 0b101
print((255).bit_count())   # 8 — 0b11111111
print((-1).bit_count())    # 1 — abs(-1) = 1 = 0b1

# bit_count on BigInt
print((2**100).bit_count())       # 1
print((2**100 - 1).bit_count())   # 100

# is_integer: always True for int
print((0).is_integer())    # True
print((42).is_integer())   # True
print((-7).is_integer())   # True

# class-method syntax: int.bit_length(n)
print(int.bit_length(5))   # 3
print(int.bit_count(5))    # 2
print(int.is_integer(42))  # True

# TypeError: wrong receiver in descriptor call
try:
    int.bit_length("hello")
except TypeError as e:
    print(e)

# TypeError: extra arguments
try:
    (5).bit_length(1)
except TypeError as e:
    print(e)

try:
    (5).bit_count(1)
except TypeError as e:
    print(e)

try:
    (5).is_integer(1)
except TypeError as e:
    print(e)
