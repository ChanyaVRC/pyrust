# Bytes literal escape sequences — recognised escapes must be handled correctly.
# Unrecognised-escape tests are in the Rust unit tests (lexer::tests) because
# CPython 3.12 emits a SyntaxWarning to stderr for them, which would cause a
# parity mismatch since pyrust has no warning infrastructure.

# Recognised single-character escapes
assert b'\n' == bytes([0x0A])
assert b'\t' == bytes([0x09])
assert b'\r' == bytes([0x0D])
assert b'\\' == bytes([0x5C])
assert b'\'' == bytes([0x27])
assert b'"'  == bytes([0x22])
assert b'\a' == bytes([0x07])
assert b'\b' == bytes([0x08])
assert b'\f' == bytes([0x0C])
assert b'\v' == bytes([0x0B])
assert b'\0' == bytes([0x00])

# Hex escape
assert b'\x00' == bytes([0x00])
assert b'\x41' == b'A'
assert b'\xFF' == bytes([0xFF])
assert b'\xff' == bytes([0xFF])

# Octal escape
assert b'\0'   == bytes([0])
assert b'\101' == b'A'           # 0o101 = 65
assert b'\377' == bytes([0xFF])  # 0o377 = 255

# Adjacent literal concatenation with escapes
assert b'\n' b'\t' == bytes([0x0A, 0x09])

# Mixed content
assert b'A\nB' == bytes([65, 10, 66])
assert b'\x41\x42\x43' == b'ABC'

print("bytes escapes OK")
