# bytearray.maketrans returns bytes (same as bytes.maketrans)
tbl = bytearray.maketrans(b'abc', b'xyz')
print(type(tbl).__name__)          # bytes
print(len(tbl))                    # 256

# End-to-end: translate with the table
result = bytearray(b'hello abc').translate(tbl)
print(result)                      # bytearray(b'hello xyz')

# Same table as bytes.maketrans
print(tbl == bytes.maketrans(b'abc', b'xyz'))  # True

# Called on an instance (should also work as static method)
tbl2 = bytearray(b'').maketrans(b'abc', b'xyz')
print(tbl2 == tbl)                 # True

# Type is exposed
print(hasattr(bytearray, 'maketrans'))  # True
