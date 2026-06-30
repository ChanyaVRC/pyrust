import re

# Basic inline flags
print(re.search(r'(?i)hello', 'HELLO').group())  # HELLO
print(re.search(r'(?m)^\d+', '123\n456').group())  # 123 (first)
print(re.findall(r'(?m)^\d+', '123\n456'))  # ['123', '456']
print(bool(re.search(r'(?s).+', 'a\nb')))  # True (dot matches newline)
print(re.search(r'(?x) h e l l o', 'hello').group())  # hello (whitespace ignored)

# Combined flags
print(re.search(r'(?im)^hello', 'HELLO\nWORLD').group())  # HELLO

# Scoped flag group (?flags:pattern)
print(re.search(r'(?i:hello) world', 'HELLO world').group())  # HELLO world
print(re.search(r'(?i:hello) world', 'HELLO WORLD'))  # None

# Scoped flag only affects its own subpattern
print(re.match(r'pre(?i:X)post', 'preXpost').group())  # preXpost
print(re.match(r'pre(?i:X)Post', 'preXpost'))  # None (Post case-sensitive)

# Flag-clearing scoped group (?flags-flags:...) and (?-flags:...)
print(re.match(r'(?i:a(?-i:b)c)', 'AbC').group())  # AbC
print(re.match(r'(?i:a(?-i:b)c)', 'ABC'))  # None (inner B is case-sensitive)
print(re.search(r'(?-s:.)', 'a\nb').group())  # a (dot does not match newline)

# Comment may precede global flags
print(re.search(r'(?#c)(?i)hi', 'HI').group())  # HI

# Inline flags from re.compile
p = re.compile(r'(?i)pattern')
print(bool(p.match('PATTERN')))  # True

# Inline flag with findall
print(re.findall(r'(?i)cat', 'Cat CAT cat'))  # ['Cat', 'CAT', 'cat']

# Error parity: global flags must come first
try:
    re.compile(r'a(?i)')
except re.error as e:
    print('err:', e)

# Error parity: missing ':' in clearing group
try:
    re.compile(r'(?i-s)')
except re.error as e:
    print('err:', e)

# Error parity: cannot turn off type flags
try:
    re.compile(r'(?-a:x)')
except re.error as e:
    print('err:', e)

# Error parity: unrecognised flag letter (alpha vs non-alpha)
try:
    re.compile(r'(?iz)')
except re.error as e:
    print('err:', e)
try:
    re.compile(r'(?i-zm:y)')
except re.error as e:
    print('err:', e)
try:
    re.compile(r'(?i9)')
except re.error as e:
    print('err:', e)

# Error parity: the same flag turned on and off
try:
    re.compile(r'(?i-i:y)')
except re.error as e:
    print('err:', e)

print("re inline flags ok")
