# Parity fixture for bytes.rfind, bytes.index, bytes.rindex — issue #696.
# These methods were absent from the dispatch table and raised AttributeError.

# --- rfind ---

print(b"hello".rfind(b"l"))           # 3
print(b"hello".rfind(b"x"))           # -1
print(b"hello".rfind(b"l", 0, 5))     # 3  explicit normal window
print(b"hello".rfind(b"l", 4, 2))     # -1  inverted window
print(b"hello".rfind(b""))            # 5  empty sub → end of range
print(b"hello".rfind(b"", 2, 4))      # 4  empty sub in mid-range
print(b"".rfind(b""))                 # 0  empty haystack + empty sub
print(b"hello".rfind(108))            # 3  integer byte value (ord('l'))

# --- index ---

print(b"hello".index(b"l"))           # 2
print(b"hello".index(b"l", 0, 5))     # 2  explicit window
print(b"hello".index(108))            # 2  integer byte value

try:
    b"hello".index(b"x")
except ValueError as e:
    print("ValueError:", e)           # ValueError: subsection not found

try:
    b"hello".index(b"l", 4, 2)
except ValueError as e:
    print("ValueError:", e)           # ValueError: subsection not found (inverted)

# --- rindex ---

print(b"hello".rindex(b"l"))          # 3
print(b"hello".rindex(b"l", 0, 5))    # 3  explicit window
print(b"hello".rindex(108))           # 3  integer byte value

try:
    b"hello".rindex(b"x")
except ValueError as e:
    print("ValueError:", e)           # ValueError: subsection not found

try:
    b"hello".rindex(b"l", 4, 2)
except ValueError as e:
    print("ValueError:", e)           # ValueError: subsection not found (inverted)

# --- hasattr sanity ---

print(hasattr(b"hello", "rfind"))     # True
print(hasattr(b"hello", "index"))     # True
print(hasattr(b"hello", "rindex"))    # True

# --- regression: existing methods still work ---

print(b"hello".find(b"l"))            # 2
print(b"hello".count(b"l"))           # 2
