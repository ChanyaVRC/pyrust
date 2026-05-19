# Raw f-strings: rf"..." / fr"..." and all case variants.
# Backslash sequences are passed through verbatim; {{ / }} still escape braces.

name = "world"

# All 8 case variants produce the same output.
print(rf"hello\n{name}")
print(rF"hello\n{name}")
print(Rf"hello\n{name}")
print(RF"hello\n{name}")
print(fr"hello\n{name}")
print(fR"hello\n{name}")
print(Fr"hello\n{name}")
print(FR"hello\n{name}")

# Double-brace escaping is preserved in raw mode.
print(rf"{{literal}}")
print(rf"}}")

# Other backslash sequences are literal.
print(rf"\t{1+1}")
print(rf"\x41{name}")

# Single-quoted raw f-string.
print(rf'\n{name}')

# Triple-quoted raw f-string.
print(rf"""multi\nline {1+1}""")

# Regular (non-raw) f-strings are unaffected.
print(f"hello\n{name}")
