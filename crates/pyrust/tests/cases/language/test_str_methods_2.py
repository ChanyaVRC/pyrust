# casefold — for ASCII identical to lower()
assert "Hello WORLD".casefold() == "hello world"
assert "".casefold() == ""
# casefold — Unicode special cases (ß→ss, ligatures)
assert "ß".casefold() == "ss"
assert "Straße".casefold() == "strasse"
assert "ﬁle".casefold() == "file"

# center — CPython places extra pad on the right for even width, left for odd:
# marg = width - len, left = marg//2 + (marg & width & 1)
assert "hi".center(6) == "  hi  "
assert "hi".center(5) == "  hi "
assert "hi".center(6, "*") == "**hi**"
assert "hello".center(3) == "hello"  # width < len → unchanged
assert "hi".center(-5) == "hi"       # negative width → unchanged

# ljust
assert "hi".ljust(5) == "hi   "
assert "hi".ljust(5, "-") == "hi---"
assert "hello".ljust(3) == "hello"
assert "hi".ljust(-3) == "hi"        # negative width → unchanged

# rjust
assert "hi".rjust(5) == "   hi"
assert "hi".rjust(5, "0") == "000hi"
assert "hello".rjust(3) == "hello"
assert "hi".rjust(-3) == "hi"        # negative width → unchanged

# zfill
assert "42".zfill(5) == "00042"
assert "-42".zfill(6) == "-00042"
assert "+42".zfill(6) == "+00042"
assert "42".zfill(2) == "42"
assert "".zfill(3) == "000"
assert "42".zfill(-1) == "42"        # negative width → unchanged

# expandtabs
assert "a\tb".expandtabs() == "a       b"
assert "a\tb".expandtabs(4) == "a   b"
assert "ab\tcd".expandtabs(4) == "ab  cd"
assert "a\tb\tc".expandtabs(0) == "abc"
assert "a\nb\tc".expandtabs(4) == "a\nb   c"

# partition
assert "hello world".partition(" ") == ("hello", " ", "world")
assert "hello".partition("x") == ("hello", "", "")
assert "aXbXc".partition("X") == ("a", "X", "bXc")

# rpartition
assert "hello world foo".rpartition(" ") == ("hello world", " ", "foo")
assert "hello".rpartition("x") == ("", "", "hello")
assert "aXbXc".rpartition("X") == ("aXb", "X", "c")

# splitlines
assert "a\nb\nc".splitlines() == ["a", "b", "c"]
assert "a\nb\nc".splitlines(True) == ["a\n", "b\n", "c"]
assert "a\r\nb\rc".splitlines() == ["a", "b", "c"]
assert "a\r\nb\rc".splitlines(True) == ["a\r\n", "b\r", "c"]
assert "".splitlines() == []
assert "a\n".splitlines() == ["a"]
assert "a\n".splitlines(True) == ["a\n"]
assert "\n\n".splitlines() == ["", ""]

# removeprefix
assert "TestHook".removeprefix("Test") == "Hook"
assert "TestHook".removeprefix("Hook") == "TestHook"
assert "".removeprefix("x") == ""

# removesuffix
assert "TestHook".removesuffix("Hook") == "Test"
assert "TestHook".removesuffix("Test") == "TestHook"
assert "".removesuffix("x") == ""

# swapcase
assert "Hello World".swapcase() == "hELLO wORLD"
assert "".swapcase() == ""
assert "123".swapcase() == "123"

# title
assert "hello world".title() == "Hello World"
assert "it's a test".title() == "It'S A Test"
assert "".title() == ""
assert "hello-world".title() == "Hello-World"

# islower
assert "hello".islower()
assert not "Hello".islower()
assert not "".islower()
assert "hello world".islower()
assert not "hello World".islower()
assert "hello123".islower()  # digits are non-cased, OK
assert not "123".islower()   # no cased chars

# isupper
assert "HELLO".isupper()
assert not "Hello".isupper()
assert not "".isupper()
assert "HELLO WORLD".isupper()
assert "HELLO123".isupper()
assert not "123".isupper()

# istitle
assert "Hello World".istitle()
assert not "hello world".istitle()
assert not "Hello world".istitle()
assert not "".istitle()
assert "Hello-World".istitle()
assert not "It's".istitle()   # apostrophe is non-cased, so 's' would need to be uppercase

# isascii
assert "hello".isascii()
assert "".isascii()
assert not "é".isascii()   # U+00E9 — non-ASCII

# isdecimal
assert "123".isdecimal()
assert not "".isdecimal()
assert not "12a".isdecimal()
assert not "½".isdecimal()   # U+00BD vulgar fraction — not decimal

# isnumeric
assert "123".isnumeric()
assert "½".isnumeric()       # U+00BD is numeric
assert not "".isnumeric()
assert not "abc".isnumeric()

# isidentifier
assert "hello".isidentifier()
assert "_hello".isidentifier()
assert "hello123".isidentifier()
assert not "".isidentifier()
assert not "123abc".isidentifier()
assert not "hello world".isidentifier()

# isprintable
assert "hello".isprintable()
assert "".isprintable()
assert not "\n".isprintable()
assert not "\t".isprintable()
assert not chr(0).isprintable()           # null control character
assert not " ".isprintable()      # line separator (Zl)
assert not " ".isprintable()      # paragraph separator (Zp)
assert not "­".isprintable()      # soft hyphen (Cf / format)
assert " ".isprintable()               # ASCII space is printable

print("str methods 2 OK")
