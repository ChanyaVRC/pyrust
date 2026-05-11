s = "Hello, World!"

# upper / lower / capitalize
assert s.upper() == "HELLO, WORLD!"
assert s.lower() == "hello, world!"
assert "hello world".capitalize() == "Hello world"

# strip / lstrip / rstrip
assert "  hi  ".strip() == "hi"
assert "  hi  ".lstrip() == "hi  "
assert "  hi  ".rstrip() == "  hi"
assert "xxhixx".strip("x") == "hi"

# split
words = "a b c".split()
assert words == ["a", "b", "c"]

words2 = "a,b,c".split(",")
assert words2 == ["a", "b", "c"]

words3 = "a,b,c".split(",", 1)
assert words3 == ["a", "b,c"]

# rsplit
words4 = "a,b,c".rsplit(",", 1)
assert words4 == ["a,b", "c"]

# join
assert ", ".join(["a", "b", "c"]) == "a, b, c"

# find / rfind
assert "abcabc".find("b") == 1
assert "abcabc".rfind("b") == 4
assert "abcabc".find("z") == -1

# index
assert "abcabc".index("b") == 1

# count
assert "abcabc".count("a") == 2
assert "hello".count("l") == 2

# replace
assert "aabbcc".replace("b", "x") == "aaxxcc"
assert "aabbcc".replace("b", "x", 1) == "aaxbcc"

# startswith / endswith
assert "hello".startswith("he")
assert not "hello".startswith("lo")
assert "hello".endswith("lo")
assert not "hello".endswith("he")

# isdigit / isalpha / isalnum / isspace
assert "123".isdigit()
assert not "12a".isdigit()
assert "abc".isalpha()
assert not "ab1".isalpha()
assert "abc123".isalnum()
assert not "abc!".isalnum()
assert "   ".isspace()
assert not " a ".isspace()

print("str methods OK")
