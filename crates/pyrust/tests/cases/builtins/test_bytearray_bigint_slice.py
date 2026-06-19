BIG = 1 << 70
ba = bytearray(b'abc')

print(ba[BIG:])         # bytearray(b'')
print(ba[:BIG])         # bytearray(b'abc')
print(ba[:-BIG])        # bytearray(b'')
print(ba[BIG:2*BIG])    # bytearray(b'')
print(ba[::BIG])        # bytearray(b'a')  (step > len, only first element)
print(ba[::-BIG])       # bytearray(b'c')  (reverse huge step, only last element)

# Negative BigInt bounds
print(ba[-BIG:])        # bytearray(b'abc')
print(ba[-BIG:-BIG])    # bytearray(b'')
print(ba[BIG::-1])      # bytearray(b'cba')
print(ba[:-BIG:-1])     # bytearray(b'cba')

# Empty bytearray with BigInt bounds (boundary)
print(bytearray(b'')[BIG:])    # bytearray(b'')
print(bytearray(b'')[::-BIG])  # bytearray(b'')

# Slice assignment / deletion with BigInt bounds
ba2 = bytearray(b'abc')
ba2[BIG:] = b'XY'
print(ba2)              # bytearray(b'abcXY')
ba2 = bytearray(b'abc')
del ba2[:BIG]
print(ba2)             # bytearray(b'')

# Normal slicing still works
print(ba[1:2])    # bytearray(b'b')
print(ba[::-1])   # bytearray(b'cba')
