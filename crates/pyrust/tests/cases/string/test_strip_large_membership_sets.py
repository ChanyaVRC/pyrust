ascii_chars = "abcdefghijklmnopqrstuvwxyz"
print(("".join(reversed(ascii_chars)) + "KEEP" + ascii_chars).strip(ascii_chars))
print(("abcabcCENTERxyzxyz").lstrip(ascii_chars))
print(("abcabcCENTERxyzxyz").rstrip(ascii_chars))
print("abcabc".strip(ascii_chars))

unicode_chars = "αβγδεζηθικλμνξοπρστυφχψω🙂🚀"
edge = "".join(reversed(unicode_chars)) + unicode_chars
print((edge + "KEEP" + edge).strip(unicode_chars))
print((edge + "RIGHT").lstrip(unicode_chars))
print(("LEFT" + edge).rstrip(unicode_chars))
print(edge.strip(unicode_chars))

# More than eight characters but only two unique members still uses set
# semantics rather than substring semantics.
duplicates = "abababababababab"
print("abbaMIDDLEbaab".strip(duplicates))

byte_chars = bytes(range(1, 64))
byte_edge = byte_chars[::-1] + byte_chars
print((byte_edge + b"\0KEEP\0" + byte_edge).strip(byte_chars))
print((byte_edge + b"\0RIGHT").lstrip(byte_chars))
print((b"LEFT\0" + byte_edge).rstrip(byte_chars))
print(byte_edge.strip(byte_chars))

print(bytearray(byte_edge + b"\0KEEP\0" + byte_edge).strip(bytearray(byte_chars)))
print(bytearray(byte_edge + b"\0RIGHT").lstrip(byte_chars))
print(bytearray(b"LEFT\0" + byte_edge).rstrip(byte_chars))
