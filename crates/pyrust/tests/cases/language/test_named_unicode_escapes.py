# Test \N{name} named Unicode escape sequences.
# Use ord() for non-ASCII output to avoid Windows CI encoding issues.

# Basic named escapes
print(ord("\N{LATIN SMALL LETTER A}"))   # 97
print(ord("\N{SNOWMAN}"))                 # 9731
print(ord("\N{LATIN SMALL LETTER A WITH ACUTE}"))  # 225

# Triple-quoted strings
print(ord("""\N{SNOWMAN}"""))            # 9731

# Multiple escapes in one string
s = "\N{LATIN SMALL LETTER A}\N{LATIN SMALL LETTER B}"
print(len(s))   # 2
print(ord(s[0]))  # 97
print(ord(s[1]))  # 98

# Mixed with other escapes
s2 = "\n\N{LATIN SMALL LETTER A}"
print(len(s2))   # 2
print(ord(s2[0]))  # 10
print(ord(s2[1]))  # 97

# NULL character
print(ord("\N{NULL}"))  # 0

# f-string with \N{name}
x = 42
print(ord(f"\N{SNOWMAN}"))            # 9731
print(f"\N{SNOWMAN}" == "\N{SNOWMAN}")  # True

# f-string: \N{name} followed by an expression
result = f"\N{SNOWMAN}{x}"
print(len(result))   # 2
print(ord(result[0]))  # 9731
print(result[1])     # 42

# Raw string: \N is literal (no escape)
print(len(r"\N{SNOWMAN}"))  # 12
print(r"\N{SNOWMAN}"[0])    # backslash

# Equality check
print("\N{LATIN SMALL LETTER A}" == "a")  # True
print("\N{SNOWMAN}" == "☃")           # True
