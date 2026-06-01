# str.upper() / str.lower() must produce byte-identical output for the
# ASCII fast path and the full-Unicode path. Covers pure ASCII, ASCII with
# digits/punctuation/spaces, empty string, and non-ASCII cases including
# multi-char expansion (ß -> SS), ligatures, Greek, and mixed ASCII+non-ASCII
# (which must route through the Unicode path).

ascii_cases = [
    "",
    "hello",
    "WORLD",
    "Hello World 123!",
    "  spaces  ",
    "MiXeD CaSe",
    "abc-DEF_ghi",
    "1234567890",
    "!@#$%^&*()",
    "i",
    "I",
]

unicode_cases = [
    "ß",
    "groß",
    "Straße",
    "İ",
    "ﬁ",
    "ﬂ ligature",
    "Ω greek Δ",
    "café",
    "naïve",
    "ǅ",
    "ǆ",
    "Hello ß World",
    "mixed İ ascii",
]

for c in ascii_cases + unicode_cases:
    print(repr(c.upper()))
    print(repr(c.lower()))
