# Parity fixture: bytes/bytearray methods accept bytes-subclass instances and
# bytearray as bytes-like arguments, matching CPython 3.12.  See issue #1928.


class B(bytes):
    pass


# Search / count (bytes receiver, bytes-subclass arg).
assert b"hello".count(B(b"l")) == 2
assert b"hello".find(B(b"ll")) == 2
assert b"hello".rfind(B(b"l")) == 3
assert b"hello".index(B(b"e")) == 1
assert b"hello".rindex(B(b"l")) == 3

# Replace.
assert b"hello".replace(B(b"l"), B(b"L")) == b"heLLo"

# Split family.
assert b"a,b,c".split(B(b",")) == [b"a", b"b", b"c"]
assert b"a b c".rsplit(B(b" ")) == [b"a", b"b", b"c"]

# Strip family.
assert b"xxhixx".strip(B(b"x")) == b"hi"
assert b"xxhixx".lstrip(B(b"x")) == b"hixx"
assert b"xxhixx".rstrip(B(b"x")) == b"xxhi"

# startswith / endswith, single and tuple forms.
assert b"hello".startswith(B(b"he")) is True
assert b"hello".endswith(B(b"lo")) is True
assert b"hello".startswith((B(b"zz"), B(b"he"))) is True
assert b"hello".endswith((B(b"zz"), B(b"lo"))) is True

# Partition.
assert b"a-b".partition(B(b"-")) == (b"a", b"-", b"b")
assert b"a-b-c".rpartition(B(b"-")) == (b"a-b", b"-", b"c")

# join: each sequence item may be a bytes subclass or bytearray.
assert b"x".join([B(b"a"), B(b"b")]) == b"axb"
assert b"x".join([bytearray(b"a"), bytearray(b"b")]) == b"axb"

# `in` / __contains__ with bytes-subclass / bytearray left operand.
assert (B(b"ll") in b"hello") is True
assert (bytearray(b"ll") in b"hello") is True
assert (B(b"zz") in b"hello") is False

# bytearray as a plain bytes-like argument into bytes methods.
assert b"hello".count(bytearray(b"l")) == 2
assert b"hello".replace(bytearray(b"l"), bytearray(b"L")) == b"heLLo"
assert b"a,b".split(bytearray(b",")) == [b"a", b"b"]
assert b"hello".startswith(bytearray(b"he")) is True

# bytearray receiver accepts bytes-subclass / bytearray args.
assert bytearray(b"hello").count(B(b"l")) == 2
assert bytearray(b"hello").replace(B(b"l"), B(b"L")) == bytearray(b"heLLo")
assert bytearray(b"hello").startswith(B(b"he")) is True
assert bytearray(b"x").join([B(b"a"), B(b"b")]) == bytearray(b"axb")
assert (B(b"ll") in bytearray(b"hello")) is True

# Integer arguments are still accepted by count/find (a byte value).
assert b"hello".count(ord("l")) == 2

# Genuinely-wrong-type arguments still raise the existing errors.
for bad in ("l", None):
    try:
        b"hello".count(bad)
        raise AssertionError("expected TypeError")
    except TypeError:
        pass

try:
    b"hello".count(300)
    raise AssertionError("expected ValueError")
except ValueError:
    pass

try:
    b"x".join([b"a", "b"])
    raise AssertionError("expected TypeError")
except TypeError:
    pass

print("ok")
