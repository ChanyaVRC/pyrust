# Unicode escape sequences (\uNNNN and \UNNNNNNNN) — parity with CPython 3.12

# \uNNNN — four hex digits, BMP codepoint
print("A")                       # A  (basic ASCII sanity check)
print(ord("é") == 0xe9)          # True
print("A" == "A")                # True

# \uNNNN round-trips
print(ord("A") == 0x0041)        # True
print("A" == "A")                # True

# \UNNNNNNNN — eight hex digits, full Unicode range
print(ord("\U00000041") == 0x41)   # True — \U of 'A'
print("\U00000041" == "A")         # True

# Emoji via \U (outside BMP)
print(ord("\U0001F600") == 0x1F600)   # True
print("\U0001F600" == "\U0001F600")   # True

# Verify é via \u
print(ord("é") == 0xe9)    # True
print("é" == "é")          # True

# \u in f-strings
print(f"A{1+1}")            # A2
print(f"{'A'}")             # A
print(f"\U00000041{2}")     # A2

# \u in triple-quoted strings
print("""\U00000041""")     # A
print("""A""")              # A

# \u and \U for the same codepoint are equal
print("A" == "\U00000041")  # True
print(ord("\U0001F600") == ord("\U0001F600"))  # True
