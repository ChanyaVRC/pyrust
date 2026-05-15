# bytes(str, encoding[, errors]) parity (#391).
#
# Validates the three-arg / two-arg overload that pyrust previously
# answered with a generic RuntimeError.  Coverage:
#   - utf-8 round-trip including a multi-byte char and a non-BMP one,
#   - ascii / latin-1 success + failure paths,
#   - encoding-name aliases (case + underscore vs hyphen, "u8", "UTF8"),
#   - errors="ignore" drops non-encodable codepoints silently,
#   - errors="replace" substitutes b'?' for unencodable codepoints,
#   - bogus encoding names raise LookupError,
#   - non-encodable bytes raise UnicodeEncodeError (subclass of
#     ValueError in CPython — verified here),
#   - exact CPython error-message wording (position + codepoint repr),
#   - the single-arg paths (no encoding) are untouched,
#   - bytes(b'…', encoding) is a TypeError (the encoding overload
#     applies only to str sources).

# ── utf-8: pure ASCII, multi-byte BMP, and non-BMP ────────────────────
assert bytes("hello", "utf-8") == b"hello"
assert bytes("héllo", "utf-8") == b"h\xc3\xa9llo"
assert bytes("", "utf-8") == b""
# Non-BMP codepoint U+10000 — encoded as a 4-byte UTF-8 sequence.
assert bytes("𐀀", "utf-8") == b"\xf0\x90\x80\x80"

# ── ascii ─────────────────────────────────────────────────────────────
assert bytes("hello", "ascii") == b"hello"
try:
    bytes("héllo", "ascii")
    print("FAIL: ascii on non-ASCII should raise")
# Exact CPython wording — \xXX codepoint repr, accurate position.
except UnicodeEncodeError as e:
    assert str(e) == (
        "'ascii' codec can't encode character '\\xe9' in position 1: "
        "ordinal not in range(128)"
    ), f"ascii msg: {e}"
    print("ascii reject:", e)

# UnicodeEncodeError is a ValueError subclass in CPython.
try:
    bytes("héllo", "ascii")
except ValueError:
    print("ascii reject is ValueError: yes")
else:
    print("FAIL: not a ValueError")

# Non-BMP codepoint should be formatted with \UXXXXXXXX in the error.
try:
    bytes("𐀀", "ascii")
    print("FAIL: ascii on non-BMP should raise")
except UnicodeEncodeError as e:
    assert str(e) == (
        "'ascii' codec can't encode character '\\U00010000' in position 0: "
        "ordinal not in range(128)"
    ), f"non-bmp msg: {e}"
    print("ascii non-bmp reject:", e)

# Position must reflect the first unencodable char, not byte offsets,
# and must not be confused by earlier-encodable repeats.
try:
    bytes("ababé", "ascii")
    print("FAIL: ababé should raise")
except UnicodeEncodeError as e:
    assert str(e) == (
        "'ascii' codec can't encode character '\\xe9' in position 4: "
        "ordinal not in range(128)"
    ), f"pos-repeat msg: {e}"
    print("ascii pos-repeat reject:", e)

# ── latin-1 ───────────────────────────────────────────────────────────
assert bytes("hello", "latin-1") == b"hello"
assert bytes("héllo", "latin-1") == b"h\xe9llo"
# A codepoint above U+00FF cannot fit in latin-1.
try:
    bytes("Ā", "latin-1")
    print("FAIL: latin-1 U+0100 should raise")
except UnicodeEncodeError as e:
    assert str(e) == (
        "'latin-1' codec can't encode character '\\u0100' in position 0: "
        "ordinal not in range(256)"
    ), f"latin-1 msg: {e}"
    print("latin-1 reject:", e)

# ── alias names (case + hyphen/underscore normalisation) ──────────────
assert bytes("hello", "UTF-8") == b"hello"
assert bytes("hello", "utf_8") == b"hello"
assert bytes("hello", "UTF8") == b"hello"
assert bytes("hello", "u8") == b"hello"
assert bytes("hello", "US-ASCII") == b"hello"
assert bytes("hello", "ISO-8859-1") == b"hello"

# ── errors="ignore" drops non-encodable bytes ─────────────────────────
assert bytes("héllo", "ascii", "ignore") == b"hllo"
# Default "strict" path still works for an all-encodable input.
assert bytes("hi", "ascii", "strict") == b"hi"

# ── errors="replace" substitutes '?' for each non-encodable codepoint
assert bytes("héllo", "ascii", "replace") == b"h?llo"
assert bytes("Āb", "latin-1", "replace") == b"?b"

# ── contiguous run of unencodable characters ──────────────────────────
# CPython groups consecutive unencodable codepoints into one error span
# rather than stopping at the first failing char.  The message uses
# "characters" (plural, no codepoint repr) and "position S-E" notation.
try:
    bytes("éé", "ascii")
    print("FAIL: éé should raise")
except UnicodeEncodeError as e:
    assert str(e) == (
        "'ascii' codec can't encode characters in position 0-1: "
        "ordinal not in range(128)"
    ), f"contiguous-run msg: {e}"
    print("contiguous-run reject:", e)

try:
    bytes("aéàb", "ascii")
    print("FAIL: aéàb should raise")
except UnicodeEncodeError as e:
    assert str(e) == (
        "'ascii' codec can't encode characters in position 1-2: "
        "ordinal not in range(128)"
    ), f"interior-run msg: {e}"
    print("interior-run reject:", e)

# ── error handler is deferred (not looked up unless needed) ──────────
# An unknown handler name on an all-encodable input must NOT raise;
# CPython only consults the error handler at the first unencodable char.
assert bytes("hi", "utf-8", "bogus") == b"hi"
assert bytes("hi", "ascii", "bogus") == b"hi"
# When an unencodable char exists, the bogus handler raises LookupError.
try:
    bytes("é", "ascii", "bogus")
    print("FAIL: should raise LookupError for bogus handler")
except LookupError as e:
    print("deferred-handler LookupError:", e)

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

# bytes(b'…', encoding) — the encoding overload is str-only.  CPython
# reports this as a plain TypeError ("encoding without a string
# argument"); we just check the type to stay forward-compatible with
# wording tweaks across CPython point releases.
try:
    bytes(b"hi", "utf-8")
    print("FAIL: bytes(bytes, encoding) should TypeError")
except TypeError:
    print("bytes+encoding TypeError: yes")

print("bytes encoding OK")
