# bytes.expandtabs() parity fixture — issue #1170
# Each print() is compared byte-for-byte against CPython 3.12 output.

# Default tabsize=8
print(repr(b"hello\tworld".expandtabs()))

# Explicit tabsize=4
print(repr(b"hello\tworld".expandtabs(4)))

# Leading and trailing tab
print(repr(b"\thello\t".expandtabs(4)))

# Multiple tabs
print(repr(b"a\tb\tc".expandtabs(4)))

# Empty bytes
print(repr(b"".expandtabs()))

# No tabs — returned unchanged
print(repr(b"no_tabs".expandtabs()))

# Consecutive tabs at tabsize=4
print(repr(b"\t\t".expandtabs(4)))

# \n resets column counter
print(repr(b"\n\t".expandtabs(4)))

# \r resets column counter
print(repr(b"a\r\t".expandtabs(4)))

# Tab at column 3 with tabsize=4 → 1 space (to reach column 4)
print(repr(b"abc\tdef".expandtabs(4)))

# tabsize=0: tabs are silently removed
print(repr(b"a\tb".expandtabs(0)))

# tabsize=1: every tab becomes a single space (tab stop at every column)
print(repr(b"a\tb".expandtabs(1)))

# Bool tabsize (True=1, False=0)
print(repr(b"a\tb".expandtabs(True)))
print(repr(b"a\tb".expandtabs(False)))

# Mixed \r\n (both reset column)
print(repr(b"a\r\nb\tc".expandtabs(4)))

# Negative tabsize treated like 0 (removes tabs)
print(repr(b"a\tb".expandtabs(-1)))

# TypeError for non-integer tabsize
try:
    b"a".expandtabs("foo")
except TypeError as e:
    print(f"TypeError: {e}")

# Too many arguments
try:
    b"a".expandtabs(4, 8)
except TypeError as e:
    print(f"TypeError: {e}")
