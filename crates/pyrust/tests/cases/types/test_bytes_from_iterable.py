# bytes() — general iterable fallback (issue #732)
# Exercises range, generator expressions, list, tuple, and bytes inputs,
# plus error paths for out-of-range and non-integer elements.

print(bytes(range(5)))       # b'\x00\x01\x02\x03\x04'
print(bytes([65, 66, 67]))   # b'ABC'
print(bytes((72, 105)))      # b'Hi'
print(bytes(b"hello"))       # b'hello'
print(bytes(0))              # b''
print(bytes(3))              # b'\x00\x00\x00'

# Generator expression
print(bytes(x for x in range(3)))   # b'\x00\x01\x02'

# Full range 0-255 round-trip length
print(len(bytes(range(256))))  # 256

# Error: value out of range (>=256)
try:
    bytes([300])
except ValueError:
    print("ValueError")

# Error: value out of range (<0)
try:
    bytes([-1])
except ValueError:
    print("ValueError")

# Error: non-int element in iterable
try:
    bytes(["a"])
except TypeError:
    print("TypeError")

# Error: value out of range from range iterable
try:
    bytes(range(256, 260))
except ValueError:
    print("ValueError")
