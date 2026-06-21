import re

# Positive lookbehind
print(re.search(r'(?<=a)b', 'ab').group())      # b
print(re.search(r'(?<=a)b', 'xb'))              # None
print(re.findall(r'(?<=\d)x', '1x2x3x'))        # ['x', 'x', 'x']

# Negative lookbehind
print(re.search(r'(?<!a)b', 'ab'))              # None
print(re.search(r'(?<!a)b', 'xb').group())      # b

# Multi-character fixed-width lookbehind
print(re.search(r'(?<=foo)bar', 'foobar').group())   # bar
print(re.search(r'(?<!foo)bar', 'bazbar').group())   # bar

# In the middle of a pattern
print(re.sub(r'(?<=\w)\.(?=\w)', '_', 'a.b.c'))  # a_b_c

# With other constructs
text = 'abc123def456'
print(re.findall(r'(?<=\D)\d+', text))           # ['123', '456']

# Word preceded by whitespace
print(re.findall(r'(?<=\s)\w+', 'hello world'))  # ['world']

# Lookbehind requires fixed width -- variable width must error
try:
    re.compile(r'(?<=a+)b')
    print("no error")
except re.error as e:
    print("error:", "look-behind" in str(e) or "fixed" in str(e))  # True

# Alternation of equal-length branches is allowed
print(re.search(r'(?<=foo|bar)x', 'barx').group())  # x

# Alternation of unequal-length branches is rejected
try:
    re.compile(r'(?<=a|bb)c')
    print("no error")
except re.error as e:
    print("alt error:", "look-behind" in str(e))  # True

# Combined lookbehind and lookahead
print(re.search(r'(?<=a)b(?=c)', 'abc').group())  # b

# Capturing group inside lookbehind
m = re.search(r'(?<=(a))b', 'ab')
print(m.group(), m.group(1))                      # b a

# Negative lookbehind does not leak captures
m = re.search(r'(?<!(x))b', 'ab')
print(m.group(), m.group(1))                      # b None

print("re lookbehind ok")
