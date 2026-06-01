# Parity tests for bytes.decode() with the UTF-16/UTF-32 LE/BE codecs.
# Exercises valid input (incl. astral chars), empty input, truncated/odd-length
# input, out-of-range code points, lone surrogates, and every error handler.
# The codec name in UnicodeDecodeError must be the correct per-endianness name.


def show(label, fn):
    try:
        print(label, "OK", repr(fn()))
    except Exception as e:
        print(label, type(e).__name__, str(e))


# Valid multi-codepoint round-trips (non-ASCII + astral needing surrogate pairs).
s = "héllo \U0001d54f 世界"
for enc in ["utf-16-le", "utf-16-be", "utf-32-le", "utf-32-be", "utf-16", "utf-32"]:
    b = s.encode(enc)
    show("valid " + enc, lambda b=b, enc=enc: b.decode(enc))
    show("empty " + enc, lambda enc=enc: b"".decode(enc))

# Odd-length UTF-16 (truncated trailing byte) under every handler.
for enc in ["utf-16-le", "utf-16-be", "utf-16"]:
    bad = b"\x00\x00\x41"
    for h in ["strict", "ignore", "replace", "backslashreplace", "surrogateescape", "bogus"]:
        show("odd " + enc + " " + h, lambda bad=bad, enc=enc, h=h: bad.decode(enc, h))

# Non-multiple-of-4 UTF-32 (truncated tail) under every handler.
for enc in ["utf-32-le", "utf-32-be", "utf-32"]:
    bad = b"\x41\x00\x00\x00\x42\x00\x00"
    for h in ["strict", "ignore", "replace", "backslashreplace", "surrogateescape", "bogus"]:
        show("trunc " + enc + " " + h, lambda bad=bad, enc=enc, h=h: bad.decode(enc, h))

# UTF-32 code point out of range(0x110000).
for enc in ["utf-32-le", "utf-32-be"]:
    bad = b"\x00\x00\x11\x00" if enc == "utf-32-le" else b"\x00\x11\x00\x00"
    for h in ["strict", "ignore", "replace", "backslashreplace", "bogus"]:
        show("oor " + enc + " " + h, lambda bad=bad, enc=enc, h=h: bad.decode(enc, h))

# Lone high surrogate (0xD800) in UTF-16.
for enc in ["utf-16-le", "utf-16-be"]:
    bad = b"\x00\xd8" if enc == "utf-16-le" else b"\xd8\x00"
    for h in ["strict", "ignore", "replace", "backslashreplace", "bogus"]:
        show("surr " + enc + " " + h, lambda bad=bad, enc=enc, h=h: bad.decode(enc, h))
