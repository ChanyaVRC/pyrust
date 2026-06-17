try:
    b"hi".hex(bytes_per_sep="x")
except TypeError as e:
    print(e)

try:
    b"hi".hex("-", "x")
except TypeError as e:
    print(e)

try:
    bytearray(b"hi").hex(bytes_per_sep=3.5)
except TypeError as e:
    print(e)

# bps without sep is otherwise unused — no error when int
print(b"hi".hex(bytes_per_sep=2))

# bytes_per_sep uses CPython's C `int` converter: a value that doesn't fit a
# 32-bit int raises OverflowError, not a TypeError, even when no sep is present.
for v in (10**30, 2**40, 2**31, -(2**40)):
    try:
        b"hi".hex(bytes_per_sep=v)
    except OverflowError as e:
        print(type(e).__name__, e)
    try:
        b"hi".hex("-", v)
    except OverflowError as e:
        print(type(e).__name__, e)

# i32 boundaries are accepted (no separator effect when groups exceed length).
print(b"hello".hex("-", 2147483647))
print(b"hello".hex("-", -2147483648))
