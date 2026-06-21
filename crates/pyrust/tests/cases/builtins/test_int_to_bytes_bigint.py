# int.to_bytes() with a BigInt length argument.
#
# The length is converted to a C ssize_t in CPython before any range/sign check,
# so a value that does not fit raises OverflowError (for both positive and
# negative BigInt lengths). The project reference is CPython 3.12, so this
# fixture is run under python3.12 by the parity harness.
big = 10**100

try:
    (1).to_bytes(big)
except OverflowError as e:
    print(str(e))

try:
    (256).to_bytes(big, 'big')
except OverflowError as e:
    print(str(e))

try:
    (1).to_bytes(big, byteorder='little')
except OverflowError as e:
    print(str(e))

# A negative BigInt length also overflows the ssize_t conversion first, so it is
# an OverflowError -- not a ValueError about non-negativity.
try:
    (1).to_bytes(-big)
except OverflowError as e:
    print(str(e))

# For contrast: valid length but value too big -> different error.
try:
    (256).to_bytes(1)
except OverflowError as e:
    print(str(e))

# Normal cases still work.
print((1).to_bytes(2, 'big').hex())
print((256).to_bytes(2, 'big').hex())
