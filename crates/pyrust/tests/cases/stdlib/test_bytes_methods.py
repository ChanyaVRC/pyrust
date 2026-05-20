# Parity fixture for bytes methods added in #829.
# All outputs verified against python3.12.

# replace
print(b"hello world".replace(b"world", b"pyrust"))  # b'hello pyrust'
print(b"aabaa".replace(b"a", b"x", 2))              # b'xxbaa'
print(b"hello".replace(b"l", b""))                  # b'heo'
print(b"hello".replace(b"", b"-", 3))               # b'-h-e-llo'

# strip / lstrip / rstrip
print(b" hello ".strip())                            # b'hello'
print(b"xxhelloxx".strip(b"x"))                     # b'hello'
print(b"  hi  ".lstrip())                           # b'hi  '
print(b"  hi  ".rstrip())                           # b'  hi'
print(b"\t\nhello\r\n".strip())                  # b'hello'

# removeprefix / removesuffix
print(b"hello world".removeprefix(b"hello "))       # b'world'
print(b"hello world".removesuffix(b" world"))       # b'hello'
print(b"hello".removeprefix(b"world"))              # b'hello'
print(b"hello".removesuffix(b"world"))              # b'hello'

try:
    b"x".removeprefix()
except TypeError as e:
    print("TypeError:", e)
try:
    b"x".removesuffix()
except TypeError as e:
    print("TypeError:", e)
try:
    b"x".removeprefix("hello")
except TypeError as e:
    print("TypeError:", e)
try:
    b"x".removesuffix(42)
except TypeError as e:
    print("TypeError:", e)

# split / rsplit
print(b"a,b,c".split(b","))                         # [b'a', b'b', b'c']
print(b"a b c".split())                              # [b'a', b'b', b'c']
print(b"a,b,c".rsplit(b",", 1))                     # [b'a,b', b'c']
print(b"a b c".rsplit(None, 1))                     # [b'a b', b'c']
print(b"".split(b","))                              # [b'']
print(b"a,,b".split(b","))                          # [b'a', b'', b'b']

# splitlines
print(b"a\nb\nc".splitlines())                     # [b'a', b'b', b'c']
print(b"a\r\nb".splitlines())                      # [b'a', b'b']
print(b"".splitlines())                              # []
print(b"a\n".splitlines(True))                     # [b'a\n']
print(b"a\r\nb\r".splitlines(True))               # [b'a\r\n', b'b\r']
# bytes.splitlines() only splits on \n and \r (unlike str.splitlines).
# \x0b, \x0c, \x1c-\x1e are NOT line boundaries for bytes.
print(b"a\x0bb".splitlines())                      # [b'a\x0bb']
print(b"a\x0cb".splitlines())                      # [b'a\x0cb']
print(b"a\x1cb".splitlines())                      # [b'a\x1cb']
print(b"a\x1db".splitlines())                      # [b'a\x1db']
print(b"a\x1eb".splitlines())                      # [b'a\x1eb']

# join
print(b",".join([b"a", b"b", b"c"]))               # b'a,b,c'
print(b"".join([b"abc", b"def"]))                   # b'abcdef'

# case methods
print(b"HELLO".lower())                              # b'hello'
print(b"hello".upper())                              # b'HELLO'
print(b"Hello".title())                              # b'Hello'
print(b"HELLO WORLD".title())                        # b'Hello World'
print(b"hello".capitalize())                         # b'Hello'
print(b"HELLO".capitalize())                         # b'Hello'

# is* methods
print(b"abc123".isdigit())                           # False
print(b"123".isdigit())                              # True
print(b"abc".isalpha())                              # True
print(b"ABC".isupper())                              # True
print(b"abc".islower())                              # True
print(b"abc123".isalnum())                           # True
print(b" \t\n".isspace())                          # True
print(b"".isdigit())                                 # False
print(b"".isalpha())                                 # False
print(b"123".isupper())                              # False
print(b"123".islower())                              # False

# center / ljust / rjust
print(b"hi".center(6))                              # b'  hi  '
print(b"hi".center(7))                              # b'   hi  '
print(b"hi".ljust(6, b"-"))                         # b'hi----'
print(b"hi".rjust(6))                               # b'    hi'
print(b"hello".center(3))                           # b'hello'

# zfill
print(b"42".zfill(5))                               # b'00042'
print(b"-42".zfill(5))                              # b'-0042'
print(b"+42".zfill(5))                              # b'+0042'
print(b"42".zfill(2))                               # b'42'

# translate
print(b"hello".translate(None, b"l"))               # b'heo'
print(b"hello".translate(None))                     # b'hello'

# decode (already existed, confirming still works)
print(b"hello".decode("utf-8"))                     # hello
print(b"hello".decode())                            # hello
