# Parity fixture for repr() of strings containing control characters.
# CPython 3.12 emits \xNN escapes for non-printable chars rather than raw bytes.

# ASCII control characters (C0 range), excluding the three named escapes
print(repr('\x00'))   # NUL
print(repr('\x01'))
print(repr('\x02'))
print(repr('\x03'))
print(repr('\x04'))
print(repr('\x05'))
print(repr('\x06'))
print(repr('\x07'))   # BEL — must be \x07, not \a
print(repr('\x08'))   # BS  — must be \x08, not \b
# \x09 (\t), \x0a (\n), \x0d (\r) are tested below for regression
print(repr('\x0b'))   # VT  — must be \x0b, not \v
print(repr('\x0c'))   # FF  — must be \x0c, not \f
print(repr('\x0e'))
print(repr('\x0f'))
print(repr('\x10'))
print(repr('\x1f'))

# DEL (U+007F)
print(repr('\x7f'))

# Named escapes must NOT change to \xNN
print(repr('\n'))     # \x0a — must remain \n
print(repr('\t'))     # \x09 — must remain \t
print(repr('\r'))     # \x0d — must remain \r

# C1 controls (U+0080-U+009F) — also non-printable in Python
print(repr('\x80'))
print(repr('\x9f'))

# U+00A0 NO-BREAK SPACE and U+00AD SOFT HYPHEN — non-printable in Python
print(repr('\xa0'))
print(repr('\xad'))

# Normal printable characters must not be escaped
print(repr('hello'))
print(repr('a\x00b'))  # mixed: control in middle of printable chars
print(repr('\x00\n\x01'))  # mix of control and named escape
