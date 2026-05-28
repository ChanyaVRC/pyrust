# Parity fixture for issue #1034: bytes 'in' operator range check.
# CPython 3.12 raises ValueError for integer operands outside 0..=255,
# not TypeError.  Bool operands (True/False) are treated as 0 and 1.

b = b'hello'

# In-range int: membership check works normally.
print(97 in b)   # True  (ord('a') == 97)
print(104 in b)  # True  (ord('h') == 104)
print(120 in b)  # False (ord('x') == 120, not in b'hello')

# Sub-bytes search still works (regression guard).
print(b'he' in b)   # True
print(b'lo' in b)   # True
print(b'xyz' in b)  # False
print(b'' in b)     # True (empty always contained)

# Bool operands: bool is a subclass of int; True==1 and False==0.
print(True in b'\x01hello')   # True  (1 is in the bytes)
print(False in b'\x00hello')  # True  (0 is in the bytes)
print(True in b'hello')       # False (1 not in b'hello')

# Out-of-range int: ValueError, not TypeError.
try:
    print(256 in b)
except ValueError as e:
    print(type(e).__name__ + ': ' + str(e))

try:
    print(-1 in b)
except ValueError as e:
    print(type(e).__name__ + ': ' + str(e))

# BigInt: also raises ValueError (always out of byte range).
try:
    print(2**100 in b)
except ValueError as e:
    print(type(e).__name__ + ': ' + str(e))

# Non-int, non-bytes: TypeError with correct message.
try:
    print('x' in b)
except TypeError as e:
    print(type(e).__name__ + ': ' + str(e))

try:
    print(3.14 in b)
except TypeError as e:
    print(type(e).__name__ + ': ' + str(e))
