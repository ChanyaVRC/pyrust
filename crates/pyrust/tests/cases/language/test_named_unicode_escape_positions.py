# Verify that \N{name} escapes work correctly at various positions in a string.
# This exercises the content_start offset logic introduced to fix issue #996.
# Error-path position messages (SyntaxError text) require compile() which is
# not yet available in pyrust; those are verified against the binary directly.

# Escape at position 0 (start of string)
print(ord("\N{SNOWMAN}"))                          # 9731

# Escape after ASCII prefix of various lengths — use [-1] to get the char
print(ord("a\N{SNOWMAN}"[-1]))                     # 9731
print(ord("abc\N{SNOWMAN}"[-1]))                   # 9731
print(ord("abcdef\N{SNOWMAN}"[-1]))                # 9731

# Escape in triple-quoted string at non-zero position
print(ord("""abc\N{SNOWMAN}"""[-1]))               # 9731
print(ord("""\N{SNOWMAN}"""))                      # 9731

# Two escapes in the same string
s = "\N{LATIN SMALL LETTER A}\N{LATIN SMALL LETTER B}"
print(ord(s[0]))   # 97
print(ord(s[1]))   # 98

# Escape following another escape sequence
s2 = "\x41\N{LATIN SMALL LETTER B}"
print(ord(s2[0]))  # 65  (A)
print(ord(s2[1]))  # 98  (b)

# NULL at position 0
print(ord("\N{NULL}"))                             # 0

# Valid escape in f-string at non-zero position
x = "!"
result = f"abc\N{SNOWMAN}{x}"
print(result[-1])       # !
print(ord(result[-2]))  # 9731
print(result[:3])       # abc
