# Tests for expanded codec support: UTF-16, UTF-32, UTF-8-SIG encodings,
# backslashreplace / surrogateescape error handlers, and Latin-1 aliases.
# Fixes #1081, #1136, #1426.

# ── str.encode() new encodings ────────────────────────────────────────────────

# UTF-16 with LE BOM
print(repr("hello".encode("utf-16")))
print(repr("hello".encode("utf_16")))

# UTF-16-LE (no BOM)
print(repr("hello".encode("utf-16-le")))
print(repr("hello".encode("utf_16_le")))

# UTF-16-BE (no BOM)
print(repr("hello".encode("utf-16-be")))
print(repr("hello".encode("utf_16_be")))

# UTF-32 with LE BOM
print(repr("hello".encode("utf-32")))
print(repr("hello".encode("utf_32")))

# UTF-32-LE (no BOM)
print(repr("hello".encode("utf-32-le")))
print(repr("hello".encode("utf_32_le")))

# UTF-32-BE (no BOM)
print(repr("hello".encode("utf-32-be")))
print(repr("hello".encode("utf_32_be")))

# UTF-8-SIG: BOM prefix
print(repr("hello".encode("utf-8-sig")))
print(repr("hello".encode("utf_8_sig")))

# Non-ASCII chars in UTF-16-LE
print(repr("héllo".encode("utf-16-le")))

# ── bytes.decode() new encodings ──────────────────────────────────────────────

# UTF-16-LE decode
print(repr(b'h\x00e\x00l\x00l\x00o\x00'.decode('utf-16-le')))
print(repr(b'h\x00e\x00l\x00l\x00o\x00'.decode('utf_16_le')))

# UTF-16 with LE BOM
print(repr(b'\xff\xfeh\x00e\x00l\x00l\x00o\x00'.decode('utf-16')))

# UTF-16 with BE BOM
print(repr(b'\xfe\xff\x00h\x00e\x00l\x00l\x00o'.decode('utf-16')))

# UTF-16-BE decode
print(repr(b'\x00h\x00e\x00l\x00l\x00o'.decode('utf-16-be')))

# UTF-32-LE decode
print(repr(b'h\x00\x00\x00e\x00\x00\x00l\x00\x00\x00l\x00\x00\x00o\x00\x00\x00'.decode('utf-32-le')))

# UTF-32 with LE BOM
print(repr(b'\xff\xfe\x00\x00h\x00\x00\x00'.decode('utf-32')))

# UTF-32-BE decode
print(repr(b'\x00\x00\x00h'.decode('utf-32-be')))

# UTF-8-SIG: strip BOM if present
print(repr(b'\xef\xbb\xbfhello'.decode('utf-8-sig')))
print(repr(b'hello'.decode('utf-8-sig')))

# Latin-1 aliases
print(repr(b'hello'.decode('l1')))
print(repr(b'hello'.decode('cp819')))
print(repr(b'hello'.decode('latin')))
print(repr(b'\x80\xff'.decode('l1')))

# ── backslashreplace error handler ────────────────────────────────────────────

# ASCII: each bad byte becomes \xNN
print(repr(b'\x80'.decode('ascii', errors='backslashreplace')))
print(repr(b'\xff'.decode('ascii', errors='backslashreplace')))
print(repr(b'hello'.decode('ascii', errors='backslashreplace')))
print(repr(b'hi\x80there'.decode('ascii', errors='backslashreplace')))

# UTF-8: each bad byte in invalid sequence becomes \xNN
print(repr(b'\xff'.decode('utf-8', errors='backslashreplace')))
print(repr(b'\xc3\x28'.decode('utf-8', errors='backslashreplace')))
print(repr(b'\xe2\x80\x28'.decode('utf-8', errors='backslashreplace')))
print(repr(b'h\xc3\xa9llo'.decode('utf-8', errors='backslashreplace')))

# ── surrogateescape error handler ─────────────────────────────────────────────

# ASCII: each bad byte becomes U+DC80 + byte
print(repr(b'\x80'.decode('ascii', errors='surrogateescape')))
print(repr(b'\xff'.decode('ascii', errors='surrogateescape')))

# UTF-8: each bad byte in invalid sequence becomes a lone surrogate
print(repr(b'\xff'.decode('utf-8', errors='surrogateescape')))
print(repr(b'\x80\x81'.decode('utf-8', errors='surrogateescape')))
print(repr(b'\xc3\x28'.decode('utf-8', errors='surrogateescape')))

# Valid UTF-8 passes through unchanged with both handlers
print(repr(b'h\xc3\xa9llo'.decode('utf-8', errors='surrogateescape')))
print(repr(b'h\xc3\xa9llo'.decode('utf-8', errors='backslashreplace')))

# ── Unknown encoding / handler still raises LookupError ───────────────────────

try:
    b'x'.decode('notanencoding')
except LookupError as e:
    print(type(e).__name__)

try:
    b'\x80'.decode('ascii', errors='notahandler')
except LookupError as e:
    print(type(e).__name__)
