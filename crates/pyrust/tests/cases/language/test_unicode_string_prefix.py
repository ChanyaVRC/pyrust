# u"..." and U"..." prefixes are plain string literals in Python 3.3+.
# They exist for Python 2/3 compatibility and produce exactly the same
# token as an unadorned string.

# Basic u prefix
print(u"hello")
print(U"world")

# type is str, just like a plain string
print(type(u"x") is str)
print(type(U"x") is str)

# Equality with plain string
print(u"hello" == "hello")
print(U"hello" == "hello")

# Unicode escape sequences work inside u-strings (same as regular strings)
print(u"\N{SNOWMAN}")
print(u"A")
print(U"α")

# Single-quoted u-strings
print(u'single')
print(U'single')

# Triple-quoted u-strings
print(u"""triple""")
print(U'''triple single quotes''')

# u-string with content that uses backslash escapes
print(u"line1\nline2")
print(u"\t" == "\t")
