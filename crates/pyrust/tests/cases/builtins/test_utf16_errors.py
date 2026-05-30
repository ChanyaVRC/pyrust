# Parity fixture for issue #1813: utf-16/utf-32 decode respects errors= handler.
# Verifies that replace/ignore/backslashreplace/surrogateescape/strict all behave
# identically to CPython 3.12 for invalid utf-16 and utf-32 sequences.

# ---------------------------------------------------------------------------
# utf-16-le: lone high surrogate (U+D800 in LE = bytes [0x00, 0xD8])
# ---------------------------------------------------------------------------

print(repr(b'\x00\xd8'.decode('utf-16-le', errors='replace')))   # '\\ufffd'
print(repr(b'\x00\xd8'.decode('utf-16-le', errors='ignore')))    # ''
print(repr(b'\x00\xd8'.decode('utf-16-le', errors='backslashreplace')))  # '\\x00\\xd8'

try:
    b'\x00\xd8'.decode('utf-16-le', errors='strict')
except UnicodeDecodeError as e:
    print(e.encoding, e.reason)  # utf-16-le unexpected end of data

try:
    b'\x00\xd8'.decode('utf-16-le', errors='surrogateescape')
except UnicodeDecodeError as e:
    print(e.encoding, e.reason)  # utf-16-le unexpected end of data

# ---------------------------------------------------------------------------
# utf-16-le: lone low surrogate (U+DC00 in LE = bytes [0x00, 0xDC])
# ---------------------------------------------------------------------------

print(repr(b'\x00\xdc'.decode('utf-16-le', errors='replace')))   # '\\ufffd'
print(repr(b'\x00\xdc'.decode('utf-16-le', errors='ignore')))    # ''
print(repr(b'\x00\xdc'.decode('utf-16-le', errors='backslashreplace')))  # '\\x00\\xdc'

# ---------------------------------------------------------------------------
# utf-16-le: high surrogate followed by non-low-surrogate (U+D800 then 'A')
# ---------------------------------------------------------------------------

print(repr(b'\x00\xd8\x41\x00'.decode('utf-16-le', errors='replace')))  # '\\ufffdA'
print(repr(b'\x00\xd8\x41\x00'.decode('utf-16-le', errors='ignore')))   # 'A'
print(repr(b'\x00\xd8\x41\x00'.decode('utf-16-le', errors='backslashreplace')))  # '\\x00\\xd8A'

# ---------------------------------------------------------------------------
# utf-16-le: truncated (odd number of bytes)
# ---------------------------------------------------------------------------

print(repr(b'\x41'.decode('utf-16-le', errors='replace')))  # '\\ufffd'
print(repr(b'\x41'.decode('utf-16-le', errors='ignore')))   # ''
print(repr(b'\x41'.decode('utf-16-le', errors='backslashreplace')))  # '\\x41'

try:
    b'\x41'.decode('utf-16-le', errors='strict')
except UnicodeDecodeError as e:
    print(e.encoding, e.reason)  # utf-16-le truncated data

# ---------------------------------------------------------------------------
# utf-16-be: lone high surrogate (U+D800 in BE = bytes [0xD8, 0x00])
# ---------------------------------------------------------------------------

print(repr(b'\xd8\x00'.decode('utf-16-be', errors='replace')))  # '\\ufffd'
print(repr(b'\xd8\x00'.decode('utf-16-be', errors='ignore')))   # ''
print(repr(b'\xd8\x00'.decode('utf-16-be', errors='backslashreplace')))  # '\\xd8\\x00'

# ---------------------------------------------------------------------------
# utf-16 (BOM-aware): LE BOM + lone high surrogate
# ---------------------------------------------------------------------------

print(repr(b'\xff\xfe\x00\xd8'.decode('utf-16', errors='replace')))  # '\\ufffd'
print(repr(b'\xff\xfe\x00\xd8'.decode('utf-16', errors='ignore')))   # ''

# ---------------------------------------------------------------------------
# utf-32-le: invalid codepoint (0x110000, which is > max valid U+10FFFF)
# [0x00, 0x00, 0x11, 0x00] in LE = 0x00110000
# ---------------------------------------------------------------------------

print(repr(b'\x00\x00\x11\x00'.decode('utf-32-le', errors='replace')))  # '\\ufffd'
print(repr(b'\x00\x00\x11\x00'.decode('utf-32-le', errors='ignore')))   # ''
print(repr(b'\x00\x00\x11\x00'.decode('utf-32-le', errors='backslashreplace')))  # '\\x00\\x00\\x11\\x00'

try:
    b'\x00\x00\x11\x00'.decode('utf-32-le', errors='strict')
except UnicodeDecodeError as e:
    print(e.encoding, e.reason)  # utf-32-le code point not in range(0x110000)

# ---------------------------------------------------------------------------
# utf-32-le: truncated (not a multiple of 4 bytes)
# ---------------------------------------------------------------------------

print(repr(b'\x01\x00'.decode('utf-32-le', errors='replace')))  # '\\ufffd'
print(repr(b'\x01\x00'.decode('utf-32-le', errors='ignore')))   # ''
print(repr(b'\x01\x00'.decode('utf-32-le', errors='backslashreplace')))  # '\\x01\\x00'

# ---------------------------------------------------------------------------
# utf-32-be: invalid codepoint (0x110000 in BE = [0x00, 0x11, 0x00, 0x00])
# ---------------------------------------------------------------------------

print(repr(b'\x00\x11\x00\x00'.decode('utf-32-be', errors='replace')))  # '\\ufffd'
print(repr(b'\x00\x11\x00\x00'.decode('utf-32-be', errors='ignore')))   # ''

# ---------------------------------------------------------------------------
# Happy path: strict on valid data should not raise
# ---------------------------------------------------------------------------

print(repr(b'\x41\x00'.decode('utf-16-le', errors='strict')))        # 'A'
print(repr(b'\x00\xd8\x00\xdc'.decode('utf-16-le', errors='strict')))  # valid surrogate pair
print(repr(b'\x41\x00\x00\x00'.decode('utf-32-le', errors='strict')))  # 'A'

# ---------------------------------------------------------------------------
# Unknown error handler always raises LookupError
# ---------------------------------------------------------------------------

try:
    b'\x00\xd8'.decode('utf-16-le', errors='no_such_handler')
except LookupError as e:
    print(type(e).__name__, str(e))  # LookupError unknown error handler name 'no_such_handler'

try:
    b'\x00\x00\x11\x00'.decode('utf-32-le', errors='no_such_handler')
except LookupError as e:
    print(type(e).__name__, str(e))  # LookupError unknown error handler name 'no_such_handler'
