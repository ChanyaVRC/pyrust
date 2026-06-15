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
