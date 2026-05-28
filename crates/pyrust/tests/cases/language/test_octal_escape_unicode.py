# Octal escape sequences in string literals: CPython 3.12 accepts values > 0xFF
# as Unicode codepoints (emitting a SyntaxWarning to stderr, which the harness strips).

# In-range octal escapes (values 0x00–0xFF) — always accepted
assert '\0' == '\x00'       # U+0000
assert '\177' == '\x7f'     # U+007F
assert '\377' == '\xff'     # U+00FF

# Out-of-range octal escapes in string literals (values 0x100–0x1FF) — accepted
# as Unicode codepoints per CPython 3.12 behaviour
assert repr('\400') == "'Ā'"   # U+0100
assert repr('\777') == "'ǿ'"   # U+01FF

# Verify the actual codepoints
assert ord('\400') == 0x100
assert ord('\777') == 0x1FF

print("octal escape unicode OK")
