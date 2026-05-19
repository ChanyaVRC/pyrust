x = """
hello
world
"""
print(repr(x))          # '\nhello\nworld\n'

y = '''first
second'''
print(repr(y))          # 'first\nsecond'

z = """no newlines"""
print(z)                # no newlines

# Escape sequences inside triple-quoted
w = """tab:\there"""
print(w)                # tab:	here

# Backslash-newline continuation within triple-quoted (drops both)
v = """line1\
line2"""
print(repr(v))          # 'line1line2'

# Quote chars inside triple-quoted
u = """she said "hello" and he said 'hi'"""
print(u)

# Single quote variant
t = '''he said "hello" and she said 'hi' '''
print(t)

# Nested triple-quote assignment after multiline string
a = """first
"""
b = """second
"""
print(repr(a + b))
