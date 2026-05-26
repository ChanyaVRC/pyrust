# Triple-quoted bytes literals: b"""...""" and b'''...'''

# Basic double-quote triple form
print(b"""hello""" == b"hello")

# Basic single-quote triple form
print(b'''world''' == b"world")

# Embedded newline in source becomes literal newline in bytes
x = b"""line1
line2"""
print(list(x))

# Single-quote triple with embedded newline
y = b'''foo
bar'''
print(list(y))

# Escape sequences work inside triple-quoted bytes
print(list(b"""\x41\n\t"""))

# Octal escape inside triple-quoted bytes
print(list(b"""\101\102"""))

# Line continuation drops backslash and newline
print(list(b"""hello\
world"""))

# Single quote character inside triple-double-quoted bytes
print(list(b"""it's here"""))

# Double quote inside triple-double-quoted bytes (single, not three)
print(list(b"""one"two"""))

# Two consecutive quotes (not three) inside triple-double-quoted bytes
print(list(b"""a""b"""))

# Raw triple-quoted bytes: backslash is literal
print(list(rb"""\n not newline"""))
print(list(rb'''\t not tab'''))

# BR prefix also works
print(list(br"""\n also literal"""))

# Empty triple-quoted bytes
print(b"""""" == b"")
print(b'''''' == b"")
