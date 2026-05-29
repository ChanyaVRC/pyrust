# parity fixture: str.encode(encoding='utf-8', errors='strict') — issue #1008

# --- default (utf-8, strict) ---
print(repr("hello".encode()))
print(repr("hello".encode('utf-8')))

# --- ascii encoding ---
print(repr("hello".encode('ascii')))

# --- latin-1 encoding ---
print(repr("hello".encode('latin-1')))
# latin-1 can encode codepoints 0x00-0xFF byte-for-byte
print(repr("\xff".encode('latin-1')))

# --- utf-8 multi-byte sequences ---
print(repr("héllo".encode('utf-8')))
# \xff encodes as two bytes in utf-8
print(repr("\xff".encode('utf-8')))

# --- errors='replace' ---
print(repr("héllo".encode('ascii', 'replace')))

# --- errors='ignore' ---
print(repr("héllo".encode('ascii', 'ignore')))

# --- strict raises UnicodeEncodeError ---
try:
    "héllo".encode('ascii')
except UnicodeEncodeError as e:
    print("UnicodeEncodeError:", str(e))

try:
    "héllo".encode('ascii', 'strict')
except UnicodeEncodeError as e:
    print("UnicodeEncodeError strict:", str(e))

# --- unknown encoding raises LookupError ---
try:
    "hello".encode('unknown-codec')
except LookupError as e:
    print("LookupError:", str(e))

# --- encoding aliases ---
print(repr("hello".encode('utf8')))
print(repr("hello".encode('us-ascii')))
print(repr("hello".encode('iso-8859-1')))

# --- keyword argument forms ---
print(repr("hello".encode(encoding='utf-8')))
print(repr("hello".encode(encoding='ascii')))
print(repr("héllo".encode(encoding='ascii', errors='ignore')))
print(repr("héllo".encode(encoding='ascii', errors='replace')))

# --- unknown errors handler raises LookupError (only when character is unencodable) ---
# ASCII-only string: handler never invoked, no error
print(repr("hello".encode("ascii", "unknown_handler")))

# --- type errors ---
try:
    "hello".encode(42)
except TypeError as e:
    print("TypeError encoding:", str(e))

try:
    "hello".encode('utf-8', 42)
except TypeError as e:
    print("TypeError errors:", str(e))

# --- return type is bytes ---
print(type("hello".encode()) is bytes)

# --- unknown errors handler raises LookupError when character IS unencodable ---
try:
    "héllo".encode("ascii", "totally_unknown_handler")
except LookupError as e:
    print("LookupError unknown handler:", str(e))
