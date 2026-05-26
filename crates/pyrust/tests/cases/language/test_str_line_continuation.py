# Backslash followed by newline inside a string literal is a line continuation:
# both the backslash and the newline are dropped, and the string continues on
# the next source line.  This matches CPython 3.12 behaviour.

# Single-quoted double-quote string, space before backslash
s1 = "hello \
world"
print(s1)

# No space before backslash
s2 = "hello\
world"
print(s2)

# Single-quoted string with single quotes
s3 = 'first \
second'
print(s3)

# Triple-quoted string
s4 = """triple \
quoted"""
print(s4)

# Triple-quoted, no space
s5 = """a\
b"""
print(s5)

# Multiple continuations in a row
s6 = "x\
y\
z"
print(s6)

# f-string
val = "world"
s7 = f"hello \
{val}"
print(s7)

# Raw string: \<newline> is kept literally (backslash + newline appear in output)
r1 = r"raw \
end"
print(r1)
