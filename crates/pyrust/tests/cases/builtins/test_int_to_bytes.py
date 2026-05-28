# to_bytes basic
print((255).to_bytes(2, 'big'))      # b'\x00\xff'
print((255).to_bytes(2, 'little'))   # b'\xff\x00'
print((0).to_bytes(1, 'big'))        # b'\x00'

# to_bytes signed
print((-1).to_bytes(2, 'big', signed=True))   # b'\xff\xff'

# to_bytes keyword args
print((255).to_bytes(length=2, byteorder='big'))   # b'\x00\xff'

# to_bytes defaults (length=1, byteorder='big')
print((1).to_bytes())    # b'\x01'
print((0).to_bytes())    # b'\x00'

# to_bytes with zero length
print((0).to_bytes(0, 'big'))   # b''

# to_bytes overflow
try:
    (256).to_bytes(1, 'big')
except OverflowError as e:
    print(type(e).__name__)

try:
    (-1).to_bytes(1, 'big')
except OverflowError as e:
    print(type(e).__name__)

# to_bytes signed overflow
try:
    (-129).to_bytes(1, 'big', signed=True)
except OverflowError as e:
    print(type(e).__name__)

# to_bytes invalid byteorder
try:
    (1).to_bytes(1, 'middle')
except ValueError as e:
    print(type(e).__name__)

# from_bytes
print(int.from_bytes(b'\x00\xff', 'big'))     # 255
print(int.from_bytes(b'\xff\x00', 'little'))  # 255
print(int.from_bytes(b'\xff\xff', 'big', signed=True))  # -1
print(int.from_bytes(b'', 'big'))             # 0

# from_bytes keyword args
print(int.from_bytes(b'\x00\xff', byteorder='big'))   # 255
print(int.from_bytes(b'\xff', signed=True))           # -1

# from_bytes as instance method (receiver ignored)
print((5).from_bytes(b'\x01', 'big'))         # 1

# from_bytes with large result (BigInt)
print(int.from_bytes(b'\x01' + b'\x00'*8, 'big'))   # 2^64 = 18446744073709551616

# as_integer_ratio
print((42).as_integer_ratio())   # (42, 1)
print((0).as_integer_ratio())    # (0, 1)
print((-5).as_integer_ratio())   # (-5, 1)

# as_integer_ratio with large int
print((2**100).as_integer_ratio())   # (big, 1)

# bit_count
print((7).bit_count())    # 3
print((0).bit_count())    # 0
print((255).bit_count())  # 8
print((-1).bit_count())   # 1 (abs(-1) = 1)

# bit_length
print((255).bit_length())  # 8
print((0).bit_length())    # 0
print((-1).bit_length())   # 1
