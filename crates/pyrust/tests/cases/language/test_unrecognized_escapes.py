# Unrecognized escape sequences — CPython 3.12 preserves backslash + char verbatim.
# (CPython 3.12 emits a DeprecationWarning; pyrust accepts without the warning.)

# Basic unrecognized single-char escapes
print(repr('\z'))   # '\\z'
print(repr('\q'))   # '\\q'
print(repr('\j'))   # '\\j'

# Multiple unrecognized escapes in one string
print(repr('\z\q'))  # '\\z\\q'

# Mix of recognized and unrecognized
print(repr('\n\z\t'))  # '\n\\z\t'

# Unrecognized escape in triple-quoted string
print(repr('''\z'''))  # '\\z'

# Unrecognized escape in f-string
x = 1
print(repr(f'\z{x}'))  # '\\z1'

# Raw strings: backslash is never processed — unrecognized escapes work as-is
print(repr(r'\z'))  # '\\z'
print(repr(r'\n'))  # '\\n' (not newline)

# Recognized escapes are unchanged
print(repr('\n'))   # '\n'
print(repr('\t'))   # '\t'
print(repr('\\'))   # '\\'

# bytes literals: unrecognized escapes are also passed through
print(repr(b'\z'))  # b'\\z'
print(repr(b'\q'))  # b'\\q'
