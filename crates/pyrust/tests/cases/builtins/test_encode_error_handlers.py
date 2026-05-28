# parity fixture: str.encode() backslashreplace / xmlcharrefreplace / namereplace

# --- backslashreplace ---
# \xHH for codepoints < 0x100
print(repr("hello\xff".encode("ascii", "backslashreplace")))
# \uHHHH for codepoints in [0x100, 0xFFFF]
print(repr("Ā".encode("ascii", "backslashreplace")))
# \UHHHHHHHH for codepoints >= 0x10000
print(repr("\U00010000".encode("ascii", "backslashreplace")))
# Encodable chars pass through unchanged
print(repr("hello".encode("ascii", "backslashreplace")))
# Mixed encodable and unencodable
print(repr("caf\xe9".encode("ascii", "backslashreplace")))
# latin-1 can encode \xff; Ā is still unencodable
print(repr("hello\xff".encode("latin-1", "backslashreplace")))
print(repr("Ā".encode("latin-1", "backslashreplace")))

# --- xmlcharrefreplace ---
# Decimal codepoint in &#NNN; form
print(repr("hello\xff".encode("ascii", "xmlcharrefreplace")))
print(repr("caf\xe9".encode("ascii", "xmlcharrefreplace")))
print(repr("Ā".encode("ascii", "xmlcharrefreplace")))
print(repr("\N{SNOWMAN}".encode("ascii", "xmlcharrefreplace")))
# Encodable chars pass through
print(repr("hello".encode("ascii", "xmlcharrefreplace")))

# --- namereplace ---
# Named character gets \N{NAME}
print(repr("\N{SNOWMAN}".encode("ascii", "namereplace")))
# Private use area: no name -> backslash fallback
print(repr("".encode("ascii", "namereplace")))
# Control char with no Unicode name -> \xHH fallback
print(repr("\x80".encode("ascii", "namereplace")))
# Encodable chars pass through
print(repr("hello".encode("ascii", "namereplace")))

# --- error propagation: unknown handler raises LookupError only when needed ---
# "x" is ASCII-encodable, so the handler is never invoked -> no error
print(repr("x".encode("ascii", "unknown_handler")))

# --- strict still works ---
try:
    "hello\xff".encode("ascii", "strict")
except UnicodeEncodeError as e:
    print("UnicodeEncodeError raised")

# --- ignore still works ---
print(repr("hello\xff".encode("ascii", "ignore")))

# --- replace still works ---
print(repr("hello\xff".encode("ascii", "replace")))
