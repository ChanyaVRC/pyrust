# Parity fixture for str.translate() with surrogate codepoints.
# CPython's str type allows lone surrogates (U+D800–U+DFFF) as codepoints;
# translate() must accept them as integer mapping values, matching CPython 3.12.

# Low surrogate boundary (U+D800)
t = str.maketrans({'a': 0xD800})
print(repr('abc'.translate(t)))  # '\ud800bc'

# High surrogate boundary (U+DFFF)
t2 = str.maketrans({'a': 0xDFFF})
print(repr('abc'.translate(t2)))  # '\udfffbc'

# Non-surrogate just before surrogate range (U+D7FF) — must still work
t3 = str.maketrans({'a': 0xD7FF})
print(repr('abc'.translate(t3)))  # '\ud7ffbc'

# Non-surrogate just after surrogate range (U+E000) — must still work
t4 = str.maketrans({'a': 0xE000})
print(repr('abc'.translate(t4)))  # '\ue000bc'

# Multiple surrogates in one call
t5 = str.maketrans({'a': 0xDC00, 'b': 0xD800})
print(repr('ab'.translate(t5)))  # '\udc00\ud800'

# Non-surrogate normal replacement still works
t6 = str.maketrans({'a': 0x41})
print('abc'.translate(t6))  # Abc

# Values outside range(0x110000) still raise ValueError
try:
    'abc'.translate({ord('a'): 0x110000})
except ValueError as e:
    print(e)

try:
    'abc'.translate({ord('a'): -1})
except ValueError as e:
    print(e)
