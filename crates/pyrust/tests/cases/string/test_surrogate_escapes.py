# Lone-surrogate \u/\U string-literal escapes (U+D800–U+DFFF).
#
# CPython's str stores lone surrogates freely; a value buildable with chr()
# must also be writable as a literal escape (issue #1893).  We avoid printing
# surrogate strings directly because encoding them to the stdout codec raises
# UnicodeEncodeError in CPython; instead we observe len/ord and equality.

# \u four-digit surrogate.
x = "\udc80"
print(len(x), ord(x))

# \U eight-digit surrogates (low and high ends of the surrogate range).
print(ord("\U0000d800"))
print(ord("\U0000dc80"))
print(ord("\udfff"))
print(ord("\ud800"))

# A literal escape must equal the chr() of the same code point.
print("\udc80" == chr(0xdc80))
print("\U0000d800" == chr(0xd800))

# Two surrogate escapes stay two separate code points; they do NOT combine
# into the astral character (Python escapes are not UTF-16 surrogate pairs).
pair = "😀"
print(len(pair), [hex(ord(c)) for c in pair])
print(pair == chr(0xd83d) + chr(0xde00))

# Mixed with ASCII keeps each surrogate as one code point.
print(len("a\udc80b"))

# Raw strings do NOT decode the escape: backslash + literal characters.
print(len(r"\udc80"))

# Non-surrogate \u / \U still decode normally.
print(ord("é"), ord("\U0001F600"))

# \x and \N are unaffected.
print(ord("\x41"))
print("\N{LATIN SMALL LETTER A}")
