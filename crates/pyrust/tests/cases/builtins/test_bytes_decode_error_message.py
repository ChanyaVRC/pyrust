"""
Parity fixture: bytes.decode() TypeError messages for non-str encoding/errors.

CPython 3.12 format:
  decode() argument 'encoding' must be str, not <type>
  decode() argument 'errors' must be str, not <type>
"""

# Positional: non-str encoding (int)
try:
    b'x'.decode(42)
except TypeError as e:
    print(f"TypeError: {e}")

# Positional: non-str encoding (float)
try:
    b'x'.decode(3.14)
except TypeError as e:
    print(f"TypeError: {e}")

# Positional: non-str encoding (bytes)
try:
    b'x'.decode(b'utf-8')
except TypeError as e:
    print(f"TypeError: {e}")

# Positional: non-str errors (encoding is ok)
try:
    b'x'.decode('utf-8', 99)
except TypeError as e:
    print(f"TypeError: {e}")

# Keyword: non-str encoding (int)
try:
    b'x'.decode(encoding=42)
except TypeError as e:
    print(f"TypeError: {e}")

# Keyword: non-str encoding (bytes)
try:
    b'x'.decode(encoding=b'utf-8')
except TypeError as e:
    print(f"TypeError: {e}")

# Keyword: non-str errors
try:
    b'x'.decode('utf-8', errors=99)
except TypeError as e:
    print(f"TypeError: {e}")

# Happy path: should succeed
print(b"hello".decode("utf-8"))
print(b"hello".decode())
print(b"hello".decode(encoding="utf-8"))
print(b"hello".decode("utf-8", "strict"))
print(b"hello".decode("utf-8", errors="strict"))
