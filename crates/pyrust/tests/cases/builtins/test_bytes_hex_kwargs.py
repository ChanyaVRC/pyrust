print(b"hi".hex(sep="-"))                       # 68-69
print(b"hello".hex(sep="-", bytes_per_sep=2))   # 68-656c-6c6f
print(bytearray(b"hi").hex(sep="-"))            # 68-69
print(b"hi".hex("-"))                           # 68-69 (positional still works)

try:
    b"hi".hex(sep=123)
except TypeError as e:
    print(e)  # object of type 'int' has no len()

# Confirm edge cases
print(b"hi".hex())                              # 6869 (no sep)
print(b"hello".hex("-"))                        # 68-65-6c-6c-6f
print(b"hello".hex("-", 2))                     # 68-656c-6c6f

# bytes_per_sep keyword without sep is a no-op (plain hex)
print(b"hi".hex(bytes_per_sep=2))               # 6869

# Negative bytes_per_sep groups from the left
print(b"hello".hex(sep="-", bytes_per_sep=-2))  # 6865-6c6c-6f

# bytearray threads both keywords too
print(bytearray(b"hello").hex(sep="-", bytes_per_sep=2))  # 68-656c-6c6f

# A keyword duplicating a positional is a TypeError
try:
    b"hi".hex("-", sep="x")
except TypeError as e:
    print(e)  # argument for hex() given by name ('sep') and position (1)

# Unknown keyword
try:
    b"hi".hex(foo=1)
except TypeError as e:
    print(e)  # 'foo' is an invalid keyword argument for hex()

# Too many arguments (all keyword)
try:
    b"hi".hex(sep="-", bytes_per_sep=2, foo=1)
except TypeError as e:
    print(e)  # hex() takes at most 2 keyword arguments (3 given)

# Too many arguments (mixed)
try:
    b"hi".hex("-", 1, bytes_per_sep=2)
except TypeError as e:
    print(e)  # hex() takes at most 2 arguments (3 given)
