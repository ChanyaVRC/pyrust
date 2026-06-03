# Parity fixture for the Unicode*Error __str__ message derivations (issue #1906).
#
# The three __str__ blocks read start/end (usize) and encoding/reason/object
# (str) from the instance attrs via shared accessors. Exercise the single-position
# branch (end == start + 1), the multi-character range branch (start-end), the
# empty range (start == end), and astral code points that pick the \U escape.

# --- single-position vs multi-character ranges ---
print(str(UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid start byte")))
print(str(UnicodeDecodeError("utf-8", b"\xff\xfe\xfd", 0, 3, "invalid")))
print(str(UnicodeEncodeError("ascii", "h\xffllo", 1, 2, "ordinal not in range(128)")))
print(str(UnicodeEncodeError("ascii", "ab\xff\xfez", 2, 4, "ordinal not in range(128)")))
print(str(UnicodeTranslateError("h\xffllo", 1, 2, "surrogates not allowed")))
print(str(UnicodeTranslateError("ab\xff\xfez", 2, 4, "invalid")))

# --- empty range (start == end): not the single-position branch ---
print(str(UnicodeDecodeError("ascii", b"abc", 1, 1, "empty")))
print(str(UnicodeEncodeError("ascii", "abc", 1, 1, "empty")))
print(str(UnicodeTranslateError("abc", 1, 1, "empty")))

# --- astral code points select the \\U escape in the message ---
print(str(UnicodeEncodeError("ascii", "\U00010348", 0, 1, "astral")))
print(str(UnicodeTranslateError("\U00010348x", 0, 1, "astral")))

# --- BMP escape (\\u) ---
print(str(UnicodeEncodeError("ascii", "€", 0, 1, "euro")))
