# Parity fixture for int(bytes) large-value → BigInt fallback (issue #1519).
# The one-arg bytes arm previously raised ValueError on i64 overflow;
# now it falls back to BigInt, matching CPython 3.12 behaviour.

# One-arg form: decimal bytes overflowing i64
print(int(b"1" * 20))    # 11111111111111111111
print(int(b"9" * 20))    # 99999999999999999999

# i64::MAX boundary — fits in i64, stays as small int
print(int(b"9223372036854775807"))

# i64::MAX + 1 — overflows i64, must become BigInt
print(int(b"9223372036854775808"))

# i64::MIN boundary
print(int(b"-9223372036854775808"))

# i64::MIN - 1 — overflows i64, must become BigInt
print(int(b"-9223372036854775809"))

# Sign prefix with large value
print(int(b"+11111111111111111111"))
print(int(b"-11111111111111111111"))

# Leading/trailing whitespace around large value
print(int(b"  99999999999999999999  "))

# Small value still works
print(int(b"42"))
print(int(b" 42 "))
print(int(b"0"))

# Error cases: malformed bytes still raise ValueError (not an overflow)
try:
    int(b"hello")
except ValueError as e:
    print(f"ValueError: {e}")

try:
    int(b"")
except ValueError as e:
    print(f"ValueError: {e}")

try:
    int(b"-")
except ValueError as e:
    print(f"ValueError: {e}")
