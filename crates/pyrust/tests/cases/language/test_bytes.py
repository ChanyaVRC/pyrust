# bytes literal and bytes type

# Basic literal
b = b"hello"
assert b == b"hello"
assert b != b"world"
assert len(b) == 5

# Empty bytes
empty = b""
assert len(empty) == 0
assert not empty

# Indexing returns an int
assert b[0] == 104    # 'h'
assert b[1] == 101    # 'e'
assert b[-1] == 111   # 'o'

# Iteration yields ints
total = 0
for x in b"abc":
    total += x
assert total == 97 + 98 + 99    # 'a' + 'b' + 'c'

# Membership: int 0..255 or sub-bytes
assert 104 in b"hello"
assert 99 not in b"hello"
assert b"ell" in b"hello"
assert b"xyz" not in b"hello"
assert b"" in b"hello"

# Equality / not-equal
assert b"abc" == b"abc"
assert b"abc" != b"abd"

# Truthiness
assert b"x"
assert not b""

# Escapes
assert b"\n" == bytes([10])
assert b"\t" == bytes([9])
assert b"\x41" == bytes([0x41])
assert b"\\" == bytes([0x5c])
assert b"\"" == bytes([0x22])
assert b"\'" == bytes([0x27])

# Adjacent literal concatenation
assert b"foo" b"bar" == b"foobar"

# Constructor forms
assert bytes() == b""
assert bytes(3) == b"\x00\x00\x00"
assert bytes([65, 66, 67]) == b"ABC"
assert bytes((10, 20, 30)) == bytes([10, 20, 30])
assert bytes(b"abc") == b"abc"

# bytes(int) with negative raises
try:
    bytes(-1)
    print("FAIL: expected ValueError")
except ValueError:
    pass

# bytes(str) without encoding raises TypeError
try:
    bytes("abc")
    print("FAIL: expected TypeError")
except TypeError:
    pass

# bytes(list_of_bad_ints) raises
try:
    bytes([256])
    print("FAIL: expected ValueError")
except ValueError:
    pass


# ─── bytes(iterable) error-class + wording parity ──────────────────────────
# Per https://docs.python.org/3/library/stdtypes.html#bytes :
#   "iterable of integers in the range 0 <= x < 256"
#   non-int element  → TypeError("'X' object cannot be interpreted as an integer")
#   out-of-range int → ValueError("bytes must be in range(0, 256)")

# List form — non-int elements
try:
    bytes([1, "x"])
except TypeError as e:
    print("list str:", e)

try:
    bytes([1, None])
except TypeError as e:
    print("list None:", e)

try:
    bytes([1, 2.5])
except TypeError as e:
    print("list float:", e)

# Tuple form — same error path
try:
    bytes((1, 2.5))
except TypeError as e:
    print("tuple float:", e)

# Out-of-range int — positive and negative
try:
    bytes([1, 2, 256])
except ValueError as e:
    print("range pos:", e)

try:
    bytes([-1])
except ValueError as e:
    print("range neg:", e)


# isinstance
assert isinstance(b"x", bytes)
assert not isinstance("x", bytes)
assert not isinstance(b"x", str)

# repr
assert repr(b"hi") == "b'hi'"
assert repr(b"") == "b''"

print("bytes OK")
