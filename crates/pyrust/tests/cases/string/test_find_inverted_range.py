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
print("hello".find("l", 2, 2))
print("hello".count("l", 2, 2))

# --- normal (non-inverted) calls: must be unaffected ---
print("hello".find("l", 2, 5))
print("hello".rfind("l", 0, 4))
print("hello".count("l", 0, 5))
print("hello".index("l", 2, 5))
