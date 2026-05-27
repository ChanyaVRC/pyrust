# bytes.__contains__ error message parity with CPython 3.12.
# When the left operand is not a bytes-like object and not an int,
# the error must be: "a bytes-like object is required, not '<type>'"
# When the left operand is an int outside 0-255, the error must be:
# "byte must be in range(0, 256)"

# str operand
try:
    result = "a" in b"hello"
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

# list operand
try:
    result = [] in b"hello"
except TypeError as e:
    print(type(e).__name__ + ": " + str(e))

# int operand: valid (byte value in range), no error
print(104 in b"hello")

# bytes operand: valid (subsequence search), no error
print(b"el" in b"hello")

# empty bytes operand: valid (empty is always contained)
print(b"" in b"hello")

# int out of range (too large): ValueError
try:
    result = 256 in b"hello"
except (TypeError, ValueError) as e:
    print(type(e).__name__ + ": " + str(e))

# int out of range (negative): ValueError
try:
    result = -1 in b"hello"
except (TypeError, ValueError) as e:
    print(type(e).__name__ + ": " + str(e))

# bigint out of range: ValueError
try:
    result = (10 ** 100) in b"hello"
except (TypeError, ValueError) as e:
    print(type(e).__name__ + ": " + str(e))

# bool operands: bool is a subclass of int, so True==1 and False==0 are valid
print(True in b"\x01hello")
print(False in b"hello")
print(True in b"hello")
