# Parity fixture for bytes methods added in #1425:
# partition, rpartition, swapcase, isascii, istitle, fromhex

# --- partition ---
print(b"hello world".partition(b" "))       # found
print(b"hello world".partition(b"x"))       # not found
print(b"hello world hello".partition(b" ")) # first occurrence
print(b"".partition(b"x"))                  # empty bytes
print(b"aXbXc".partition(b"X"))             # multi-char not sep

# --- rpartition ---
print(b"hello world".rpartition(b" "))        # found
print(b"hello world".rpartition(b"x"))        # not found
print(b"hello world hello".rpartition(b" "))  # last occurrence
print(b"".rpartition(b"x"))                   # empty bytes

# --- partition / rpartition: multi-byte sep ---
print(b"aXXbXXc".partition(b"XX"))   # multi-byte sep
print(b"aXXbXXc".rpartition(b"XX"))  # multi-byte sep right

# --- partition / rpartition errors ---
try:
    b"hello".partition(b"")
except ValueError as e:
    print("ValueError:", e)
try:
    b"hello".rpartition(b"")
except ValueError as e:
    print("ValueError:", e)
try:
    b"hello".partition("x")
except TypeError as e:
    print("TypeError:", e)

# --- swapcase ---
print(b"Hello World 123".swapcase())
print(b"hELLO".swapcase())
print(b"".swapcase())
print(b"123!@#".swapcase())  # non-alpha unchanged

# --- isascii ---
print(b"hello".isascii())
print(b"".isascii())          # empty is True
print(b"\x7f".isascii())      # 0x7f is ASCII
print(b"\x80".isascii())      # 0x80 is not ASCII
print(b"abc\xff".isascii())

# --- istitle ---
print(b"Hello World".istitle())
print(b"Hello world".istitle())  # lowercase second word
print(b"hello World".istitle())  # lowercase first word
print(b"".istitle())             # empty is False
print(b"Hello".istitle())
print(b"HELLO".istitle())
print(b"Hello123".istitle())     # digits don't reset word
print(b"Hello 123".istitle())    # space+digit is ok
print(b"Hello1World".istitle())  # digit starts new word
print(b"1Hello".istitle())       # leading digit

# --- fromhex ---
print(bytes.fromhex("68656c6c6f"))        # basic
print(bytes.fromhex("68 65 6c 6c 6f"))   # spaces allowed
print(bytes.fromhex(""))                  # empty
print(bytes.fromhex("48"))               # single byte
print(bytes.fromhex("68\t65"))           # tab whitespace
print(bytes.fromhex("68\n65"))           # newline whitespace

# fromhex errors
try:
    bytes.fromhex("xyz")
except ValueError as e:
    print("ValueError:", e)
try:
    bytes.fromhex("6")
except ValueError as e:
    print("ValueError:", e)
try:
    bytes.fromhex("6g")
except ValueError as e:
    print("ValueError:", e)
try:
    bytes.fromhex(123)
except TypeError as e:
    print("TypeError:", e)
try:
    bytes.fromhex()
except TypeError as e:
    print("TypeError:", e)
try:
    bytes.fromhex("48", "extra")
except TypeError as e:
    print("TypeError:", e)

# fromhex accessible on instance too
print(b"".fromhex("48656c6c6f"))
