# Octal escapes > 0xFF in bytes literals: CPython 3.12 truncates to the low byte
# (emitting SyntaxWarning to stderr, which the parity harness ignores).

# In-range bytes octal — must still work correctly
assert b'\0' == b'\x00'      # 0 octal = 0
assert b'\377' == b'\xff'    # 377 octal = 255

# Out-of-range bytes octal — truncated to low byte
# \400 = 256 decimal; 256 % 256 = 0 → b'\x00'
assert b'\400' == b'\x00'
# \777 = 511 decimal; 511 % 256 = 255 → b'\xff'
assert b'\777' == b'\xff'

# Verify values explicitly
assert b'\400'[0] == 0
assert b'\777'[0] == 255

# Multi-byte literal with an overflow escape
data = b'\400\777'
assert len(data) == 2
assert data[0] == 0
assert data[1] == 255

print("byte octal escape overflow OK")
