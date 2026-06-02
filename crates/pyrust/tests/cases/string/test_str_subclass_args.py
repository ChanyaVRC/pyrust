# Parity fixture: str methods accept str-subclass instances as arguments,
# matching CPython 3.12 (an isinstance relationship).  See issue #1927.


class S(str):
    pass


# Search / count.
assert "hello".count(S("l")) == 2
assert "hello".find(S("ll")) == 2
assert "hello".rfind(S("l")) == 3
assert "hello".index(S("e")) == 1
assert "hello".rindex(S("l")) == 3

# Replace.
assert "hello".replace(S("l"), S("L")) == "heLLo"
assert "hello".replace(S("l"), "L") == "heLLo"
assert "hello".replace("l", S("L")) == "heLLo"

# Split family.
assert "a,b,c".split(S(",")) == ["a", "b", "c"]
assert "a b c".rsplit(S(" ")) == ["a", "b", "c"]

# Strip family.
assert "xxhixx".strip(S("x")) == "hi"
assert "xxhixx".lstrip(S("x")) == "hixx"
assert "xxhixx".rstrip(S("x")) == "xxhi"

# startswith / endswith, single and tuple-of-prefixes forms.
assert "hello".startswith(S("he")) is True
assert "hello".endswith(S("lo")) is True
assert "hello".startswith((S("zz"), S("he"))) is True
assert "hello".endswith((S("zz"), S("lo"))) is True
assert "hello".startswith((S("zz"), "he")) is True

# Partition.
assert "a-b".partition(S("-")) == ("a", "-", "b")
assert "a-b-c".rpartition(S("-")) == ("a-b", "-", "c")

# removeprefix / removesuffix.
assert "hello".removeprefix(S("he")) == "llo"
assert "hello".removesuffix(S("lo")) == "hel"

# join: each sequence item may be a str subclass.
assert "x".join([S("a"), S("b")]) == "axb"
assert "x".join((S("a"), S("b"))) == "axb"
assert "x".join([S("a"), "b"]) == "axb"

# `in` / __contains__ with a str-subclass left operand.
assert (S("ll") in "hello") is True
assert (S("zz") in "hello") is False

# Genuinely-wrong-type arguments still raise the existing TypeError.
for bad in (5, ["l"], None, b"l"):
    try:
        "hello".count(bad)
        raise AssertionError("expected TypeError")
    except TypeError:
        pass

try:
    "x".join(["a", 2])
    raise AssertionError("expected TypeError")
except TypeError:
    pass

print("ok")
