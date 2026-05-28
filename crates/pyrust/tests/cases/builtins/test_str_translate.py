# Parity fixture for str.translate() and str.maketrans() — issue #1010.

# maketrans with two equal-length strings
t = str.maketrans('aeiou', 'AEIOU')
print('hello world'.translate(t))          # hEllO wOrld

# maketrans with dict (ord key → str replacement, None deletion, int ordinal)
t2 = str.maketrans({ord('a'): 'A', ord('e'): None, ord('i'): '1'})
print('airline'.translate(t2))             # A1rl1n

# maketrans with deletechars (3-arg form)
t3 = str.maketrans('', '', 'aeiou')
print('hello world'.translate(t3))         # hll wrld

# translate with raw ord-keyed dict
print('abc'.translate({97: '!', 98: None}))  # !c

# translate with no substitutions for a char (keep as-is)
print('xyz'.translate({ord('x'): 'X'}))    # Xyz

# empty string
print(''.translate(t))                     # (empty line)
