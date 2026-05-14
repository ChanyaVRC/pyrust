# bytes(str, encoding[, errors]) parity (#391).
#
# Validates the three-arg / two-arg overload that pyrust previously
# answered with a generic RuntimeError.  Coverage:
#   - utf-8 round-trip including a multi-byte char,
#   - ascii / latin-1 success + failure paths,
#   - encoding-name aliases (case + underscore vs hyphen),
#   - errors="ignore" drops non-encodable codepoints silently,
#   - bogus encoding names raise LookupError,
#   - non-encodable bytes raise UnicodeEncodeError (subclass of
#     ValueError in CPython — verified here),
#   - the single-arg paths (no encoding) are untouched.

# ── utf-8: pure ASCII and multi-byte ──────────────────────────────────
assert bytes("hello", "utf-8") == b"hello"
assert bytes("héllo", "utf-8") == b"h\xc3\xa9llo"
assert bytes("", "utf-8") == b""

# ── ascii ─────────────────────────────────────────────────────────────
assert bytes("hello", "ascii") == b"hello"
try:
    bytes("héllo", "ascii")
    print("FAIL: ascii on non-ASCII should raise")
except UnicodeEncodeError as e:
    print("ascii reject:", e)

# UnicodeEncodeError is a ValueError subclass in CPython.
try:
    bytes("héllo", "ascii")
except ValueError:
    print("ascii reject is ValueError: yes")
else:
    print("FAIL: not a ValueError")

# ── latin-1 ───────────────────────────────────────────────────────────
assert bytes("hello", "latin-1") == b"hello"
assert bytes("héllo", "latin-1") == b"h\xe9llo"
# A codepoint above U+00FF cannot fit in latin-1.
try:
    bytes("Ā", "latin-1")
    print("FAIL: latin-1 U+0100 should raise")
except UnicodeEncodeError as e:
    print("latin-1 reject:", e)

# ── alias names (case + hyphen/underscore normalisation) ──────────────
assert bytes("hello", "UTF-8") == b"hello"
assert bytes("hello", "utf_8") == b"hello"
assert bytes("hello", "US-ASCII") == b"hello"
assert bytes("hello", "ISO-8859-1") == b"hello"

# ── errors="ignore" drops non-encodable bytes ─────────────────────────
assert bytes("héllo", "ascii", "ignore") == b"hllo"
# Default "strict" path still works for an all-encodable input.
assert bytes("hi", "ascii", "strict") == b"hi"

# ── invalid encoding name → LookupError ───────────────────────────────
try:
    bytes("hello", "definitely-not-a-codec")
    print("FAIL: bad codec should raise")
except LookupError as e:
    print("lookup:", e)

# ── single-arg paths still work ───────────────────────────────────────
assert bytes() == b""
assert bytes(3) == b"\x00\x00\x00"
assert bytes([65, 66, 67]) == b"ABC"
assert bytes(b"abc") == b"abc"

# bytes(str) without encoding still raises TypeError — explicit, since
# the new two-arg overload mustn't shadow this.
try:
    bytes("abc")
    print("FAIL: bytes(str) should TypeError")
except TypeError:
    print("no-encoding TypeError: yes")

print("bytes encoding OK")
