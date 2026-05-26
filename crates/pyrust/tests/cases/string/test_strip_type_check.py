# Parity fixture: str.strip/lstrip/rstrip raise TypeError for non-str/None chars.

# Happy paths — all should work without error.
assert "  hello  ".strip() == "hello"
assert "  hello  ".strip(None) == "hello"
assert "hello".strip("hlo") == "e"
assert "hello".lstrip("he") == "llo"
assert "hello".rstrip("lo") == "he"
print("happy paths ok")

# strip() with invalid chars type must raise TypeError.
try:
    "x".strip(42)
    print("FAIL: strip(int) should raise TypeError")
except TypeError as e:
    assert "strip arg must be None or str" in str(e), repr(str(e))
    print("strip(int) TypeError ok")

# lstrip() with invalid chars type must raise TypeError.
try:
    "x".lstrip(b"x")
    print("FAIL: lstrip(bytes) should raise TypeError")
except TypeError as e:
    assert "lstrip arg must be None or str" in str(e), repr(str(e))
    print("lstrip(bytes) TypeError ok")

# rstrip() with invalid chars type must raise TypeError.
try:
    "x".rstrip([])
    print("FAIL: rstrip(list) should raise TypeError")
except TypeError as e:
    assert "rstrip arg must be None or str" in str(e), repr(str(e))
    print("rstrip(list) TypeError ok")
