s = "".join(["abcdefghij"] * 100)

print(
    "str-prefix",
    s.startswith("abc"),
    s.endswith("hij"),
    s.startswith("bcd", 1),
    s.endswith("ghi", 0, -1),
    s.startswith(("missing", "abc")),
    s.endswith(("missing", "hij")),
)
print(
    "str-ascii-methods",
    s.isascii(),
    s.isalpha(),
    s.islower(),
    s.upper().isupper(),
    s.title().istitle(),
    s.center(len(s) + 2, "-").startswith("-"),
)
print(
    "str-unicode-methods",
    "Straße".casefold(),
    "Straße".swapcase(),
    "Straße".isidentifier(),
    "１２３".isnumeric(),
)
