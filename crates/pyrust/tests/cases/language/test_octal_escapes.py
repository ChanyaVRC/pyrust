# Octal escape sequences (\ooo) — parity with CPython 3.12

# 1-digit octal
print(ord("\0"))     # 0   — NUL
print(ord("\1"))     # 1
print(ord("\7"))     # 7

# 2-digit octal
print(ord("\12"))    # 10  — LF
print(ord("\17"))    # 15
print(ord("\77"))    # 63  — '?'

# 3-digit octal
print(ord("\101"))   # 65  — 'A'
print(ord("\060"))   # 48  — '0'
print(ord("\377"))   # 255 — 0xFF

# octal stops at 3 digits (4th digit is literal)
s = "\1234"
print(len(s))        # 2  — '\123' (83) + '4'
print(ord(s[0]))     # 83
print(s[1])          # '4'

# octal and named escapes compare equal
print("\12" == "\n")   # True
print("\0" == "\x00")  # True
print("\101" == "A")   # True

# octal in triple-quoted strings
print(ord("""\0"""))   # 0

# octal in f-strings
x = "!"
print(f"\101{x}")      # A!

# octal in bytes literals
print(b'\0')           # b'\x00'
print(b'\1')           # b'\x01'
print(b'\7')           # b'\x07'
print(b'\12')          # b'\n'
print(b'\101')         # b'A'
print(b'\377')         # b'\xff'

# 4-digit truncation in bytes
bs = b'\1234'
print(len(bs))         # 2
print(bs[0])           # 83
print(bs[1])           # 52 (ord('4'))
