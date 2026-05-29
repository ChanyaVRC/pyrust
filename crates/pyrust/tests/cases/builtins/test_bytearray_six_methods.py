# Parity fixture for the six bytearray methods added by PR #1467 to bytes
# but initially absent from bytearray (issue #1476):
#   partition, rpartition, swapcase, isascii, istitle, fromhex

ba = bytearray(b"Hello World")

# swapcase
print(ba.swapcase())

# isascii — True for pure ASCII bytes
print(ba.isascii())
print(bytearray(b"\xff").isascii())

# istitle
print(ba.istitle())
print(bytearray(b"hello").istitle())
print(bytearray(b"Hello world").istitle())

# partition — returns 3-tuple of bytearray
result = ba.partition(b" ")
print(result)
print(type(result[0]).__name__)
print(type(result[1]).__name__)
print(type(result[2]).__name__)

# partition — separator not found
print(bytearray(b"nospace").partition(b" "))

# rpartition
print(ba.rpartition(b" "))
print(bytearray(b"nospace").rpartition(b" "))

# fromhex — classmethod on the type
print(bytearray.fromhex("48656c6c6f"))
print(bytearray.fromhex("deadbeef"))
print(bytearray.fromhex("4865 6c6c 6f"))   # spaces allowed

# fromhex — accessible on instances too (CPython: b''.fromhex(s) works)
print(bytearray(b"").fromhex("48656c6c6f"))

# fromhex — result type
result = bytearray.fromhex("48656c6c6f")
print(type(result).__name__)
print(isinstance(result, bytearray))

# fromhex error cases
try:
    bytearray.fromhex("xyz")
except ValueError as e:
    print(type(e).__name__)

try:
    bytearray.fromhex(42)
except TypeError as e:
    print(type(e).__name__)
