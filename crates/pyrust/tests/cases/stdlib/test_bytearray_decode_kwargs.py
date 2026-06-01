# bytearray.decode() encoding/errors keyword-argument parity (#1980).
#
# bytearray.decode() previously dropped its kwargs on the floor, so
# bytearray(b"\xff").decode(errors="replace") raised UnicodeDecodeError
# instead of returning the U+FFFD replacement char.  bytes.decode()
# already threaded encoding/errors through; bytearray now mirrors it.
#
# Coverage: keyword, positional, mixed, and default forms; the error
# handlers replace/ignore/strict/backslashreplace; encodings
# latin-1/utf-8/ascii; plus the TypeError / LookupError edge cases that
# must match bytes.decode() (and CPython) exactly.

# ── errors= by keyword ─────────────────────────────────────────────────
assert bytearray(b"\xff").decode(errors="replace") == "�"
assert bytearray(b"\xff").decode(errors="ignore") == ""
assert bytearray(b"a\xffb").decode(errors="backslashreplace") == "a\\xffb"

# ── encoding= by keyword ───────────────────────────────────────────────
assert bytearray(b"\xff").decode(encoding="latin-1") == "ÿ"
assert bytearray(b"\xe9").decode(encoding="latin-1") == "é"
assert bytearray(b"hi").decode(encoding="ascii") == "hi"

# ── both keywords ──────────────────────────────────────────────────────
assert bytearray(b"\xff").decode(encoding="ascii", errors="replace") == "�"
assert bytearray(b"hi").decode(encoding="ascii", errors="strict") == "hi"

# ── positional (unchanged) ─────────────────────────────────────────────
assert bytearray(b"\xff").decode("latin-1") == "ÿ"
assert bytearray(b"\xff").decode("utf-8", "ignore") == ""
assert bytearray(b"\xff").decode("ascii", "replace") == "�"

# ── mixed positional encoding + keyword errors ─────────────────────────
assert bytearray(b"\xff").decode("ascii", errors="replace") == "�"

# ── defaults (utf-8 / strict) ──────────────────────────────────────────
assert bytearray(b"hello").decode() == "hello"
assert bytearray(b"h\xc3\xa9llo").decode() == "héllo"
try:
    bytearray(b"\xff").decode()
    print("FAIL: default strict should raise")
except UnicodeDecodeError as e:
    print("strict reject:", e)

# ── bytes contrast row: must behave identically ────────────────────────
assert bytes(b"\xff").decode(errors="replace") == "�"
assert bytes(b"\xff").decode(encoding="latin-1") == "ÿ"
assert bytearray(b"\xff").decode(errors="replace") == bytes(b"\xff").decode(
    errors="replace"
)

# ── error parity with bytes.decode() ───────────────────────────────────
# Unknown keyword.
try:
    bytearray(b"\xff").decode(bogus="x")
    print("FAIL: unknown kwarg should raise")
except TypeError as e:
    print("unknown kwarg:", e)

# Too many arguments.
try:
    bytearray(b"\xff").decode("utf-8", "strict", "extra")
    print("FAIL: 3 args should raise")
except TypeError as e:
    print("too many args:", e)

# Name + position clash.
try:
    bytearray(b"\xff").decode("utf-8", encoding="ascii")
    print("FAIL: name+position clash should raise")
except TypeError as e:
    print("name/position clash:", e)

# Non-str encoding.
try:
    bytearray(b"\xff").decode(encoding=5)
    print("FAIL: int encoding should raise")
except TypeError as e:
    print("bad encoding type:", e)

# Unknown codec.
try:
    bytearray(b"\xff").decode("definitely-not-a-codec")
    print("FAIL: bad codec should raise")
except LookupError as e:
    print("bad codec:", e)

# Unknown error handler (only consulted on an actual decode failure).
try:
    bytearray(b"\xff").decode("utf-8", "boguserrors")
    print("FAIL: bogus handler should raise")
except LookupError as e:
    print("bogus handler:", e)

print("bytearray decode kwargs OK")
