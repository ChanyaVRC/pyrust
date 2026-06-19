BIG = 1 << 70
ba = bytearray(b'abcdef')

print(ba[::BIG])     # bytearray(b'a')   - huge positive step = only first
print(ba[::-BIG])    # bytearray(b'f')   - huge negative step = only last
print(ba[2::BIG])    # bytearray(b'c')   - start at 2, huge step
print(ba[-1::-BIG])  # bytearray(b'f')   - from last, huge neg step
print(ba[1::BIG])    # bytearray(b'b')   - start at 1, huge step
print(ba[4::-BIG])   # bytearray(b'e')   - start at 4, huge neg step

# Slice assignment with BigInt step
ba2 = bytearray(b'abcdef')
ba2[::BIG] = bytearray(b'X')
print(ba2)           # bytearray(b'Xbcdef')

# Extended-slice assignment with non-zero start + BigInt step
ba3 = bytearray(b'abcdef')
ba3[2::BIG] = bytearray(b'Z')
print(ba3)           # bytearray(b'abZdef')

# Extended-slice deletion with non-zero start + BigInt step
ba4 = bytearray(b'abcdef')
del ba4[2::BIG]
print(ba4)           # bytearray(b'abdef')

# Normal step unaffected
print(ba[::2])       # bytearray(b'ace')
print(ba[::-1])      # bytearray(b'fedcba')
print(ba[1:5:2])     # bytearray(b'bd')
