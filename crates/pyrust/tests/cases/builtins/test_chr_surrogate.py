# chr() with surrogate codepoints (0xD800-0xDFFF) must succeed, matching CPython.
# Surrogates are not valid Unicode scalar values but CPython's str type allows them.

# Basic case -- no regression
print(repr(chr(65)))        # 'A'
print(repr(chr(0)))         # '\x00'

# Just below the surrogate range -- normal char
print(repr(chr(0xD7FF)))    # '퟿'

# Surrogate range -- must succeed, not raise ValueError
print(repr(chr(0xD800)))    # '\ud800'
print(repr(chr(0xD900)))    # '\ud900'
print(repr(chr(0xDFFF)))    # '\udfff'

# Just above the surrogate range -- normal char
print(repr(chr(0xE000)))    # ''

# Max valid codepoint
print(repr(chr(0x10FFFF)))  # '\U0010ffff'

# Out-of-range must still raise ValueError
try:
    chr(0x110000)
except ValueError as e:
    print(e)

try:
    chr(-1)
except ValueError as e:
    print(e)
