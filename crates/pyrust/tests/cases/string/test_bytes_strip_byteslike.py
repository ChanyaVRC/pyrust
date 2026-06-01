# Parity fixture: bytes/bytearray strip/lstrip/rstrip accept any bytes-like
# chars argument (bytes or bytearray), matching CPython 3.12.

# bytes receiver, bytes chars
assert b"xxabcxx".strip(b"x") == b"abc"
assert b"xxabcxx".lstrip(b"x") == b"abcxx"
assert b"xxabcxx".rstrip(b"x") == b"xxabc"

# bytes receiver, bytearray chars
assert b"xxabcxx".strip(bytearray(b"x")) == b"abc"
assert b"xxabcxx".lstrip(bytearray(b"x")) == b"abcxx"
assert b"xxabcxx".rstrip(bytearray(b"x")) == b"xxabc"

# bytearray receiver, bytes chars
assert bytearray(b"xxabcxx").strip(b"x") == bytearray(b"abc")
assert bytearray(b"xxabcxx").lstrip(b"x") == bytearray(b"abcxx")
assert bytearray(b"xxabcxx").rstrip(b"x") == bytearray(b"xxabc")

# bytearray receiver, bytearray chars
assert bytearray(b"xxabcxx").strip(bytearray(b"x")) == bytearray(b"abc")
assert bytearray(b"xxabcxx").lstrip(bytearray(b"x")) == bytearray(b"abcxx")
assert bytearray(b"xxabcxx").rstrip(bytearray(b"x")) == bytearray(b"xxabc")

# chars is a *set* of bytes, not a substring
assert b"xyxabcyxy".strip(bytearray(b"xy")) == b"abc"
assert b"xyxabcyxy".strip(b"yx") == b"abc"

# None / no-arg whitespace strip unchanged
assert b"  abc \t\n".strip() == b"abc"
assert b"  abc \t\n".strip(None) == b"abc"
assert bytearray(b"\t abc \n").strip() == bytearray(b"abc")
assert bytearray(b" abc ").lstrip() == bytearray(b"abc ")
print("happy paths ok")

# Wrong types still raise TypeError with CPython's exact message.
try:
    b"x".strip("y")
    print("FAIL: bytes.strip(str) should raise TypeError")
except TypeError as e:
    assert str(e) == "a bytes-like object is required, not 'str'", repr(str(e))
    print("bytes.strip(str) TypeError ok")

try:
    bytearray(b"x").lstrip(5)
    print("FAIL: bytearray.lstrip(int) should raise TypeError")
except TypeError as e:
    assert str(e) == "a bytes-like object is required, not 'int'", repr(str(e))
    print("bytearray.lstrip(int) TypeError ok")
