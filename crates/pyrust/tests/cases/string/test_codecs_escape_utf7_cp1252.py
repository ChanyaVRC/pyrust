# parity fixture (#1951): unicode_escape / raw_unicode_escape / utf-7 / cp1252
# encode + decode + error handlers, byte-for-byte against CPython 3.12.


def show(fn):
    try:
        print("OK", repr(fn()))
    except Exception as e:
        attrs = ""
        if isinstance(e, (UnicodeEncodeError, UnicodeDecodeError)):
            attrs = " start=%d end=%d reason=%r" % (e.start, e.end, e.reason)
        print("ERR", type(e).__name__, repr(str(e)) + attrs)


# --- unicode_escape encode ---
print(repr("café\n".encode("unicode_escape")))
print(repr("a\nb\t\r\\".encode("unicode_escape")))
print(repr("\x00\x01\x7f".encode("unicode_escape")))
print(repr("ÿĀ\U0001F600".encode("unicode_escape")))
print(repr("'\"".encode("unicode_escape")))
# name normalization (case / hyphen / underscore)
print(repr("x".encode("Unicode-Escape")))
print(repr("x".encode("UNICODE_ESCAPE")))

# --- unicode_escape decode ---
print(repr(b"caf\\xe9\\n".decode("unicode_escape")))
print(repr(b"a\\nb\\t\\r\\\\".decode("unicode_escape")))
print(repr(b"\\x00\\u0100\\U0001f600".decode("unicode_escape")))
print(repr(b"\\777\\0".decode("unicode_escape")))  # octal
print(repr(b"\\a\\b\\f\\v".decode("unicode_escape")))
print(repr(b"a\\qb".decode("unicode_escape")))  # unknown escape kept
print(repr(b"\\'\\\"".decode("unicode_escape")))

# --- unicode_escape \N{NAME} named escapes ---
print(repr(b"\\N{BULLET}".decode("unicode_escape")))
show(lambda: b"\\N{NO SUCH NAME XYZ}".decode("unicode_escape"))
show(lambda: b"\\N".decode("unicode_escape"))
show(lambda: b"\\N{BULLET".decode("unicode_escape"))
# raw_unicode_escape does NOT interpret \N
print(repr(b"\\N{BULLET}".decode("raw_unicode_escape")))

# --- unicode_escape decode errors ---
show(lambda: b"\\x".decode("unicode_escape"))
show(lambda: b"\\u12".decode("unicode_escape"))
show(lambda: b"\\U0011FFFF".decode("unicode_escape"))
show(lambda: b"\\".decode("unicode_escape"))

# --- raw_unicode_escape encode ---
print(repr("café\n".encode("raw_unicode_escape")))
print(repr("Ā\U0001F600".encode("raw_unicode_escape")))
print(repr("a\\nb".encode("raw_unicode_escape")))
print(repr("\x00\x7f\xff".encode("raw_unicode_escape")))

# --- raw_unicode_escape decode ---
print(repr(b"caf\xe9\n".decode("raw_unicode_escape")))
print(repr(b"\\u0100\\U0001f600".decode("raw_unicode_escape")))
print(repr(b"a\\nb".decode("raw_unicode_escape")))  # \n stays literal
print(repr(b"\\\\u0041".decode("raw_unicode_escape")))  # even backslashes literal
show(lambda: b"\\U0011FFFF".decode("raw_unicode_escape"))

# --- utf-7 encode ---
print(repr("+".encode("utf-7")))
print(repr("café".encode("utf-7")))
print(repr("hi+".encode("utf-7")))
print(repr("a b\tc".encode("utf-7")))
print(repr("\\~".encode("utf-7")))
print(repr("中文".encode("utf-7")))
print(repr("\U0001F600".encode("utf-7")))
print(repr("!@#$%".encode("utf-7")))

# --- utf-7 decode ---
print(repr(b"+-".decode("utf-7")))
print(repr(b"caf+AOk-".decode("utf-7")))
print(repr(b"hi+-".decode("utf-7")))
print(repr(b"+Ti1lhw-".decode("utf-7")))
print(repr(b"+AFw-".decode("utf-7")))

# --- utf-7 malformed shift sequences ---
show(lambda: b"+A-".decode("utf-7"))
show(lambda: b"+ABC-".decode("utf-7"))
show(lambda: b"+AOkA-".decode("utf-7"))
print(repr(b"+ABC-".decode("utf-7", "replace")))
print(repr(b"a+A-b".decode("utf-7", "ignore")))

# --- utf-7 round trips ---
print(repr("café中\U0001F600".encode("utf-7").decode("utf-7")))
print(repr("こんにちは世界".encode("utf-7").decode("utf-7")))

# --- cp1252 encode ---
print(repr("€".encode("cp1252")))
print(repr("abc".encode("cp1252")))
print(repr("“’".encode("cp1252")))
print(repr("x€y".encode("windows-1252")))

# --- cp1252 decode ---
print(repr(b"\x80".decode("cp1252")))
print(repr(b"abc".decode("cp1252")))
print(repr(b"\x93\x92".decode("cp1252")))

# --- cp1252 error handlers (undefined bytes) ---
show(lambda: "\x81".encode("cp1252"))
print(repr("\x81".encode("cp1252", "replace")))
print(repr("\x81".encode("cp1252", "ignore")))
show(lambda: b"\x81".decode("cp1252"))
print(repr(b"\x81".decode("cp1252", "replace")))

# --- cp1252 round trip of all defined high chars ---
high = "".join(
    chr(c)
    for c in [
        0x20AC, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030,
        0x0160, 0x2039, 0x0152, 0x017D, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x017E, 0x0178,
    ]
)
print(repr(high.encode("cp1252")))
print(high.encode("cp1252").decode("cp1252") == high)

# --- unknown codec still raises LookupError ---
show(lambda: "x".encode("definitely-not-a-codec"))
show(lambda: b"x".decode("definitely-not-a-codec"))
