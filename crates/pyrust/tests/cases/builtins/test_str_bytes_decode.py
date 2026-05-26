# str(bytes, encoding[, errors]) — 2/3-arg decoding form

# Basic utf-8 decode
print(str(b"hello", "utf-8"))
print(str(b"hello", "utf-8", "strict"))
print(str(b"hello", "utf8"))     # alias
print(str(b"hello", "utf_8"))    # alias

# ascii
print(str(b"hello", "ascii"))
print(str(b"hello", "ascii", "strict"))

# latin-1: byte 0xe9 is U+00E9
latin1_byte = bytes([0xe9])
print(ord(str(latin1_byte, "latin-1")))      # 233
print(ord(str(latin1_byte, "iso-8859-1")))   # 233
print(ord(str(latin1_byte, "iso_8859_1")))   # 233

# utf-8 multibyte (snowman U+2603)
snowman = b"\xe2\x98\x83"
decoded = str(snowman, "utf-8")
print(len(decoded))          # 1
print(ord(decoded[0]))       # 9731

# error handlers: replace
bad = bytes([0xff])
replaced = str(bad, "utf-8", "replace")
print(len(replaced))         # 1
print(ord(replaced[0]))      # 65533 (U+FFFD REPLACEMENT CHARACTER)

# error handlers: ignore
ignored = str(bad, "utf-8", "ignore")
print(len(ignored))          # 0
print(ignored)               # (empty)

# error handlers: strict raises UnicodeDecodeError
try:
    str(bad, "utf-8", "strict")
except UnicodeDecodeError:
    print("UnicodeDecodeError raised")

# ascii strict with high byte
try:
    str(bytes([0x80]), "ascii", "strict")
except UnicodeDecodeError:
    print("UnicodeDecodeError ascii raised")

# ascii replace
ascii_replaced = str(bytes([0x41, 0x80, 0x42]), "ascii", "replace")
print(len(ascii_replaced))   # 3
print(ord(ascii_replaced[0]))  # 65 (A)
print(ord(ascii_replaced[1]))  # 65533 (replacement)
print(ord(ascii_replaced[2]))  # 66 (B)

# ascii ignore
ascii_ignored = str(bytes([0x41, 0x80, 0x42]), "ascii", "ignore")
print(ascii_ignored)         # AB

# str(str, encoding) raises TypeError
try:
    str("hello", "utf-8")
except TypeError as e:
    print("TypeError for str arg:", e)

# str(int, encoding) raises TypeError
try:
    str(42, "utf-8")
except TypeError as e:
    print("TypeError for int arg:", e)

# str(list, encoding) raises TypeError
try:
    str([1, 2, 3], "utf-8")
except TypeError as e:
    print("TypeError for list arg:", e)

# too many args
try:
    str(b"hello", "utf-8", "strict", "extra")
except TypeError as e:
    print("TypeError for 4 args:", e)

# error handler is only validated when it is actually invoked (lazy, like CPython)
# valid bytes + unknown handler -> succeeds (handler never called)
print(str(b"hello", "utf-8", "bad-handler"))   # hello
print(str(b"hello", "ascii", "bad-handler"))    # hello
# latin-1 never fails, so handler is never called
print(str(bytes([0xff]), "latin-1", "bad-handler"))  # ÿ

# invalid bytes + unknown handler -> LookupError (handler is called)
try:
    str(bytes([0xff]), "utf-8", "bad-handler")
except LookupError as e:
    print("LookupError utf-8:", e)

try:
    str(bytes([0x80]), "ascii", "bad-handler")
except LookupError as e:
    print("LookupError ascii:", e)

# no regression: 0/1-arg forms still work
print(str())
print(str(42))
print(str("hello"))
print(str(True))
