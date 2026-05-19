# Tests for str.find / rfind / index / count with inverted (start > stop) windows.
# CPython 3.12 treats an inverted window as an empty search range rather than panicking.

# --- inverted window: start > stop ---

# find: empty range => not found => -1
print("hello".find("l", 4, 2))

# rfind: empty range => not found => -1
print("hello".rfind("l", 4, 2))

# count: empty range => 0
print("hello".count("l", 4, 2))

# index: empty range => ValueError
try:
    "hello".index("l", 4, 2)
except ValueError:
    print("ValueError")

# Unicode string with inverted window
print("héllo".find("l", 4, 2))
print("héllo".rfind("l", 4, 2))
print("héllo".count("l", 4, 2))
try:
    "héllo".index("l", 4, 2)
except ValueError:
    print("ValueError")

# --- equal start == stop (zero-length range) ---
# An equal-window is NOT the same as an inverted window:
# find("") at equal-window returns the position; count("") returns 1.
print("hello".find("l", 2, 2))
print("hello".count("l", 2, 2))
print("hello".find("", 2, 2))
print("hello".count("", 2, 2))

# --- empty substring with inverted window ---
# CPython: find("") => -1, count("") => 0 (inverted is distinct from zero-length)
print("hello".find("", 4, 2))
print("hello".count("", 4, 2))
print("héllo".find("", 4, 2))
print("héllo".count("", 4, 2))

# --- startswith / endswith with inverted window ---
# CPython: inverted window always yields False, even with empty prefix/suffix.
# (An empty prefix matches any string, but an inverted window has no valid slice.)
print("hello".startswith("", 5, 2))   # False  (inverted, empty prefix)
print("hello".startswith("h", 5, 2))  # False  (inverted, non-empty prefix)
print("hello".endswith("", 5, 2))     # False  (inverted, empty suffix)
print("hello".endswith("o", 5, 2))    # False  (inverted, non-empty suffix)
# Zero-length window (start == stop) is NOT inverted:
print("hello".startswith("", 2, 2))   # True   (empty prefix matches empty slice)
print("hello".endswith("", 2, 2))     # True

# --- normal (non-inverted) calls: must be unaffected ---
print("hello".find("l", 2, 5))
print("hello".rfind("l", 0, 4))
print("hello".count("l", 0, 5))
print("hello".index("l", 2, 5))
