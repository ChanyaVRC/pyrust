# String escape sequences — parity with CPython 3.12

# \x hex escapes (U+0000–U+00FF)
print("\x41")        # A
print("\x61")        # a
print(ord("\xff"))   # 255 — avoids Windows console encoding issues
print(len("\x00"))   # 1 — null character, length 1

# \x and its named counterpart produce the same character
print("\x0a" == "\n")   # True
print("\x09" == "\t")   # True
print("\x5c" == "\\")   # True

# \x in triple-quoted strings
print("""\x41 \x61""")   # A a

# \x in f-strings
x = 66
print(f"\x41{x}")        # A66
