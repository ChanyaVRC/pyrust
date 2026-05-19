# Raw string literals: r"...", R"...", rb"...", br"...", and uppercase variants.

# Basic raw string: backslash sequences are NOT expanded
print(r"hello\nworld")       # hello\nworld
print(len(r"\n"))             # 2
print(r'\t' == '\\t')        # True
print(repr(r"abc"))           # 'abc'
print(repr(r"\n\t"))          # '\\n\\t'

# Uppercase R prefix
print(repr(R"hello\n"))       # 'hello\\n'

# Raw bytes: rb"..." and br"..." — all prefix combos
print(repr(rb"\x00"))         # b'\\x00'
print(repr(br"\x00"))         # b'\\x00'
print(repr(Rb"\x00"))         # b'\\x00'
print(repr(bR"\x00"))         # b'\\x00'
print(repr(RB"\x00"))         # b'\\x00'
print(repr(BR"\x00"))         # b'\\x00'

# Backslash before quote character: prevents termination, backslash is kept
s = r"\""
print(len(s))                 # 2  (backslash + double-quote)
print(s[0] == '\\')          # True
print(s[1] == '"')            # True

# Single-line triple-quoted raw strings
print(repr(r"""line1\nline2"""))  # 'line1\\nline2'
print(repr(r'''\t\r\n'''))        # '\\t\\r\\n'

# Normal strings are unaffected
print(repr("\n"))             # '\n'
print(repr(b"\x00"))          # b'\x00'
print(repr("\t"))             # '\t'
