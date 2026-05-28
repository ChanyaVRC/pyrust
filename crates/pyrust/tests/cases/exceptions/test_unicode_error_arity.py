# Parity fixture for UnicodeDecodeError / UnicodeEncodeError /
# UnicodeTranslateError __init__ arity enforcement (issue #1227).
#
# CPython 3.12 raises TypeError when these are called with the wrong
# number of arguments.  Correct-arity calls must still succeed.

# --- UnicodeDecodeError requires exactly 5 args ---
try:
    UnicodeDecodeError("utf-8")
except TypeError as e:
    print(e)

try:
    UnicodeDecodeError("utf-8", b"abc", 0, 1)
except TypeError as e:
    print(e)

try:
    UnicodeDecodeError("utf-8", b"abc", 0, 1, "r", "extra")
except TypeError as e:
    print(e)

# Correct arity succeeds
e = UnicodeDecodeError("utf-8", b"hello", 0, 1, "reason")
print("decode ok")

# --- UnicodeEncodeError requires exactly 5 args ---
try:
    UnicodeEncodeError("utf-8")
except TypeError as e:
    print(e)

try:
    UnicodeEncodeError("ascii", "x", 0, 1)
except TypeError as e:
    print(e)

try:
    UnicodeEncodeError("utf-8", "hello", 0, 5, "reason", "extra")
except TypeError as e:
    print(e)

# Correct arity succeeds
e = UnicodeEncodeError("utf-8", "hello", 0, 5, "reason")
print("encode ok")

# --- UnicodeTranslateError requires exactly 4 args ---
try:
    UnicodeTranslateError("x")
except TypeError as e:
    print(e)

try:
    UnicodeTranslateError("abc", 0, 1)
except TypeError as e:
    print(e)

try:
    UnicodeTranslateError("hello", 0, 1, "reason", "extra")
except TypeError as e:
    print(e)

# Correct arity succeeds
e = UnicodeTranslateError("hello", 0, 1, "reason")
print("translate ok")
