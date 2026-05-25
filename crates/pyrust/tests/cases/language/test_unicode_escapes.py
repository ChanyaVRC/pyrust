# Unicode escape sequences (\uNNNN and \UNNNNNNNN) -- parity with CPython 3.12

# \uNNNN -- four hex digits, BMP codepoints
print(ord("\u0041") == 0x41)   # True -- 'A'
print("\u0041" == "A")         # True
print(ord("\u00e9") == 0xe9)   # True -- e-acute
print(ord("\u03b1") == 0x3b1)  # True -- Greek small alpha

# Multiple \u in one string
print("\u0041\u0042\u0043" == "ABC")  # True

# \UNNNNNNNN -- eight hex digits, full Unicode range
print(ord("\U00000041") == 0x41)      # True -- \U of 'A'
print("\U00000041" == "A")            # True
print(ord("\U0001F600") == 0x1F600)   # True -- emoji outside BMP
print("\U0001F600" == "\U0001F600")   # True

# \u and \U denote the same codepoint when digits match
print("\u0041" == "\U00000041")            # True
print(ord("\u0041") == ord("\U00000041"))  # True

# \u in f-strings
print(f"\u0041{1+1}")          # A2
print(f"\U00000041{2}")        # A2

# \u in triple-quoted strings
print("""\u0041""")             # A
print("""\U00000041""")         # A

# \u and \U for the same codepoint are equal
print("\u0041" == "\U00000041")  # True
print(ord("\U0001F600") == ord("\U0001F600"))  # True
