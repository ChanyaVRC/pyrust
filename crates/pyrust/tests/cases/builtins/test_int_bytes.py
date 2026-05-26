# int() accepts bytes (and treats them like str) — CPython 3.12 parity.

# Happy path: no explicit base (base 10 by default)
print(int(b"42"))          # 42
print(int(b"  42  "))      # 42  (whitespace stripped)
print(int(b"-7"))          # -7
print(int(b"+3"))          # 3

# Happy path: explicit base
print(int(b"42", 10))      # 42
print(int(b"FF", 16))      # 255
print(int(b"0xff", 16))    # 255
print(int(b"0xFF", 16))    # 255
print(int(b"0b1010", 2))   # 10
print(int(b"101", 2))      # 5
print(int(b"0o77", 8))     # 63
print(int(b"77", 8))       # 63
print(int(b"  FF  ", 16))  # 255  (whitespace stripped)

# Happy path: base 0 (auto-detect from prefix)
print(int(b"0xff", 0))     # 255
print(int(b"0b1010", 0))   # 10
print(int(b"0o77", 0))     # 63
print(int(b"42", 0))       # 42

# Error: invalid literal — base 10, no explicit base
try:
    int(b"hello")
except ValueError as e:
    print(e)

# Error: invalid literal — base 10, explicit base
try:
    int(b"hello", 10)
except ValueError as e:
    print(e)

# Error: invalid literal — base 16
try:
    int(b"hello", 16)
except ValueError as e:
    print(e)

# Error: invalid literal — base 0
try:
    int(b"  hello  ", 0)
except ValueError as e:
    print(e)

# Error: non-ASCII byte (no explicit base)
try:
    int(b"\xff")
except ValueError as e:
    print(e)

# Error: non-ASCII byte (explicit base)
try:
    int(b"\xff", 16)
except ValueError as e:
    print(e)

# No regression: str and int paths still work
print(int("42"))            # 42
print(int(42.7))            # 42
print(int("0xff", 16))      # 255
print(int())                # 0
