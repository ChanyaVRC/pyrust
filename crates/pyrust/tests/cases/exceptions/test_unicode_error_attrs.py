# Parity fixture for UnicodeDecodeError / UnicodeEncodeError /
# UnicodeTranslateError constructor attributes (issue #1377).
#
# CPython 3.12 sets five named attributes (encoding, object, start, end,
# reason) when these exceptions are constructed; str() derives the message
# from those attributes rather than from raw args.

# --- UnicodeDecodeError ---
e = UnicodeDecodeError("utf-8", b"\xff\xfe", 0, 2, "invalid start byte")
print(e.encoding)
print(e.object)
print(e.start)
print(e.end)
print(e.reason)
print(str(e))
print(e.args)

# Single-position decode error
e2 = UnicodeDecodeError("ascii", b"\x80", 0, 1, "ordinal not in range(128)")
print(str(e2))

# --- UnicodeEncodeError ---
e3 = UnicodeEncodeError("ascii", "hello\xff", 5, 6, "ordinal not in range(128)")
print(e3.encoding)
print(ascii(e3.object))
print(e3.start)
print(e3.end)
print(e3.reason)
print(str(e3))

# Multi-character encode error
e4 = UnicodeEncodeError("ascii", "\xff\xfe", 0, 2, "ordinal not in range(128)")
print(str(e4))

# --- UnicodeTranslateError ---
e5 = UnicodeTranslateError("\xff", 0, 1, "surrogates not allowed")
print(ascii(e5.object))
print(e5.start)
print(e5.end)
print(e5.reason)
print(str(e5))

# Multi-character translate error
e6 = UnicodeTranslateError("\xff\xfe", 0, 2, "invalid")
print(str(e6))

# --- internally-raised decode error ---
try:
    b"\xff".decode("utf-8")
except UnicodeDecodeError as ex:
    print(ex.encoding)
    print(ex.object)
    print(ex.start)
    print(ex.end)
    print(ex.reason)
    print(str(ex))

try:
    b"\xff".decode("ascii")
except UnicodeDecodeError as ex:
    print(ex.encoding)
    print(ex.object)
    print(ex.start)
    print(ex.end)
    print(ex.reason)
    print(str(ex))

# --- internally-raised encode error ---
# Primary repro for issue #1037: multi-byte string with a non-ASCII character.
# "caf\xe9" is 4 chars; '\xe9' (U+00E9 'e' with acute) is at index 3, so start=3, end=4.
try:
    "caf\xe9".encode("ascii")
except UnicodeEncodeError as ex:
    print(ex.encoding)
    print(ascii(ex.object))
    print(ex.start)
    print(ex.end)
    print(ex.reason)
    print(str(ex))

try:
    "\xff".encode("ascii")
except UnicodeEncodeError as ex:
    print(ex.encoding)
    print(ascii(ex.object))
    print(ex.start)
    print(ex.end)
    print(ex.reason)
    print(str(ex))

try:
    "\xff\xfe".encode("ascii")
except UnicodeEncodeError as ex:
    print(ex.start)
    print(ex.end)
    print(str(ex))

# --- TypeError on wrong arg count ---
try:
    UnicodeDecodeError("utf-8", b"abc", 0, 1)
except TypeError as ex:
    print(ex)

try:
    UnicodeDecodeError("utf-8", b"abc", 0, 1, "r", "extra")
except TypeError as ex:
    print(ex)

try:
    UnicodeTranslateError("abc", 0, 1)
except TypeError as ex:
    print(ex)

try:
    UnicodeEncodeError("ascii", "x", 0, 1)
except TypeError as ex:
    print(ex)

# --- TypeError on wrong arg types ---
try:
    UnicodeDecodeError(42, b"abc", 0, 1, "r")
except TypeError as ex:
    print(ex)

try:
    UnicodeDecodeError("utf-8", "notbytes", 0, 1, "r")
except TypeError as ex:
    print(ex)

try:
    UnicodeDecodeError("utf-8", b"abc", "bad", 1, "r")
except TypeError as ex:
    print(ex)

# --- subclass inherits attributes ---
class MyDecodeError(UnicodeDecodeError):
    pass

e7 = MyDecodeError("utf-8", b"\xff", 0, 1, "bad byte")
print(e7.encoding)
print(e7.reason)
print(str(e7))
