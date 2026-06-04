# Parity fixture for #2110: bytearray negative-step slicing must use an
# exclusive stop index (standard Python slice semantics), matching CPython and
# matching what bytes/list/tuple/str do. The previous bytearray get_item path
# included the stop index (off-by-one).

ba = bytearray(b"abcdefghij")

# The documented repro: stop is exclusive.
print(bytes(ba[8:2:-1]))  # b'ihgfed'

# Full reverse / strided reverse.
print(bytes(ba[::-1]))  # b'jihgfedcba'
print(bytes(ba[::-2]))  # b'jhfdb'

# Negative indices.
print(bytes(ba[-1:-5:-1]))  # b'jihg'
print(bytes(ba[-1:-100:-1]))  # b'jihgfedcba'

# Empty results.
print(bytes(ba[8:8:-1]))  # b''
print(bytes(ba[0:5:-1]))  # b''
print(bytes(ba[2:2:1]))  # b''

# Positive step unchanged.
print(bytes(ba[2:7]))  # b'cdefg'
print(bytes(ba[::2]))  # b'acegi'

# Out-of-range bounds clamp.
print(bytes(ba[100:-100:-1]))  # b'jihgfedcba'
print(bytes(ba[-100:100:1]))  # b'abcdefghij'

# bytearray must agree with bytes byte-for-byte.
b = b"abcdefghij"
print(bytes(ba[8:2:-1]) == b[8:2:-1])  # True
print(bytes(ba[::-1]) == b[::-1])  # True
print(bytes(ba[3:9:-2]) == b[3:9:-2])  # True

# Slice assignment with extended (negative) step still works.
ba2 = bytearray(b"abcdefghij")
ba2[8:2:-1] = b"XYZWVU"
print(bytes(ba2))  # b'abcUVWZYXj'

# Slice deletion with negative step.
ba3 = bytearray(b"abcdefghij")
del ba3[8:2:-1]
print(bytes(ba3))  # b'abcj'

ba4 = bytearray(b"abcdefghij")
del ba4[::-2]
print(bytes(ba4))  # b'acegi'
