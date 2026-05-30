# Parity fixture for bytearray concatenation and repetition (issue #1791).

# --- binary + ---
print(bytearray(b"a") + bytearray(b"b"))   # bytearray(b'ab')
print(bytearray(b"ab") + bytearray(b""))   # bytearray(b'ab')
print(bytearray(b"") + bytearray(b""))     # bytearray(b'')

# bytearray + bytes → bytearray
print(bytearray(b"a") + b"b")              # bytearray(b'ab')

# bytes + bytearray → bytes
print(b"a" + bytearray(b"b"))              # b'ab'

# --- binary * ---
print(bytearray(b"ab") * 3)               # bytearray(b'ababab')
print(3 * bytearray(b"ab"))               # bytearray(b'ababab')
print(bytearray(b"ab") * 0)               # bytearray(b'')
print(bytearray(b"ab") * -1)              # bytearray(b'')
print(bytearray(b"") * 5)                 # bytearray(b'')
print(bytearray(b"x") * 1)               # bytearray(b'x')

# --- augmented += (mutates in place, preserves object identity) ---
a = bytearray(b"a")
b = a
a += bytearray(b"b")
print(a)         # bytearray(b'ab')
print(a is b)    # True  — alias sees the mutation

a = bytearray(b"a")
b = a
a += b"bc"
print(a)         # bytearray(b'abc')
print(a is b)    # True

# --- augmented *= (mutates in place, preserves object identity) ---
a = bytearray(b"ab")
b = a
a *= 3
print(a)         # bytearray(b'ababab')
print(a is b)    # True

a = bytearray(b"ab")
b = a
a *= 0
print(a)         # bytearray(b'')
print(a is b)    # True

# --- TypeError for invalid operands ---
try:
    bytearray(b"a") + 42
except TypeError as e:
    print(e)

try:
    bytearray(b"a") * "x"
except TypeError as e:
    print(e)

try:
    bytearray(b"a") + "x"
except TypeError as e:
    print(e)

# --- OverflowError for BigInt repetition count ---
try:
    bytearray(b"a") * (2 ** 100)
except OverflowError as e:
    print(e)

try:
    (2 ** 100) * bytearray(b"a")
except OverflowError as e:
    print(e)
