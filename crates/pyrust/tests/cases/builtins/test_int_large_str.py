# Parity fixture for int() large-string → BigInt fallback (issue #1512).
# All cases that previously raised ValueError on overflow now return the correct
# large integer, matching CPython 3.12 behaviour.

# One-arg form: decimal string overflowing i64
print(int("1" * 20))    # 11111111111111111111
print(int("10" * 15))   # 101010101010101010101010101010
print(int("9" * 30))    # 30-digit integer

# Two-arg form: explicit base 10 overflowing i64
print(int("11111111111111111111", 10))   # same as first case above

# Two-arg form: explicit base 16 (hex) overflowing i64
print(int("ffffffffffffffffffff", 16))   # 1208925819614629174706175

# Two-arg form: explicit base 2 (binary) overflowing i64
print(int("1" * 60, 2))  # 1152921504606846975

# Two-arg form: base 0 (already fixed, regression guard)
print(int("0xffffffffffffffffffff", 0))  # 1208925819614629174706175

# Small int still works
print(int("12345"))   # 12345
print(int("abc", 16)) # 2748

# Two-arg bytes form: explicit base overflowing i64
print(int(b"ffffffffffffffffffff", 16))  # 1208925819614629174706175
print(int(b"11111111111111111111", 10))  # 11111111111111111111
print(int(b"0xffffffffffffffffffff", 0)) # 1208925819614629174706175

# Error cases: malformed string still raises ValueError (not overflow)
try:
    int("hello")
except ValueError as e:
    print(f"ValueError: {e}")

try:
    int("1.5")
except ValueError as e:
    print(f"ValueError: {e}")

try:
    int("not_a_number_123XYZ")
except ValueError as e:
    print(f"ValueError: {e}")
