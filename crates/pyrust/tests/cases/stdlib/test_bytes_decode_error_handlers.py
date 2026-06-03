# Exercises the shared UTF-8 error-handler decode loop behind the
# strict / ignore / replace / backslashreplace / surrogateescape handlers.
# Covers valid runs, lone invalid bytes, multi-byte invalid runs, and
# truncated trailing sequences (error_len() == None).
#
# surrogateescape decodes invalid bytes to lone surrogates (U+DC80..U+DCFF).
# Printing such a str re-encodes it and diverges across runtimes, so we compare
# the decoded code points by ordinal instead of printing the raw string.

cases = [
    b"",
    b"hello",
    bytes([0xFF]),
    bytes([0x80]),
    bytes([0x80, 0x81, 0x82]),
    b"a" + bytes([0xFF]) + b"b",
    bytes([0xE2, 0x82]),              # truncated 3-byte sequence
    bytes([0xC3, 0x28]),             # invalid continuation
    "héllo".encode("utf-8") + bytes([0xFF]),
    bytes([0xF0, 0x9F, 0x98]),       # truncated emoji
    bytes([0xED, 0xA0, 0x80]),       # surrogate range -> invalid UTF-8
    bytes(range(0x80, 0x90)),
]

for handler in ["ignore", "replace", "strict", "backslashreplace"]:
    for c in cases:
        try:
            print(handler, c, "->", c.decode("utf-8", errors=handler))
        except UnicodeDecodeError as e:
            print(handler, c, "-> ERR", e.encoding, e.object, e.start, e.end, e.reason)

# surrogateescape: compare code points by ordinal (printing lone surrogates is
# not byte-stable across runtimes).
for c in cases:
    decoded = c.decode("utf-8", errors="surrogateescape")
    print("surrogateescape", c, "->", [ord(ch) for ch in decoded])

# bytearray path shares the same decoder.
for handler in ["ignore", "backslashreplace"]:
    print(handler, bytearray([0xC3, 0x28, 0xFF]).decode("utf-8", errors=handler))
print("surrogateescape", [ord(ch) for ch in bytearray([0xC3, 0x28, 0xFF]).decode("utf-8", errors="surrogateescape")])
