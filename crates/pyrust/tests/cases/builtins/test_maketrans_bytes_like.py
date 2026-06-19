# bytes.maketrans with bytearray args
tbl = bytes.maketrans(bytearray(b'abc'), bytearray(b'xyz'))
print(type(tbl).__name__)   # bytes
print(tbl[ord('a')])        # 120  (ord('x'))

# Mixed: bytes from, bytearray to
tbl2 = bytes.maketrans(b'abc', bytearray(b'xyz'))
print(tbl2 == tbl)          # True

# Mixed: bytearray from, bytes to
tbl3 = bytes.maketrans(bytearray(b'abc'), b'xyz')
print(tbl3 == tbl)          # True

# bytearray.maketrans with bytearray args
tbl4 = bytearray.maketrans(bytearray(b'abc'), bytearray(b'xyz'))
print(tbl4 == tbl)          # True

# Mismatched lengths still ValueError
try:
    bytes.maketrans(bytearray(b'abc'), bytearray(b'xy'))
    print('WRONG')
except ValueError:
    print('ok')

# Wrong type still TypeError
try:
    bytes.maketrans('abc', 'xyz')
    print('WRONG')
except TypeError:
    print('ok')

# Original bytes-only still works (regression)
print(bytes.maketrans(b'abc', b'xyz') == tbl)  # True

# bytearray subclass accepted as either argument (CPython treats it bytes-like)
class MBA(bytearray):
    pass

print(bytes.maketrans(MBA(b'abc'), b'xyz') == tbl)        # True
print(bytes.maketrans(b'abc', MBA(b'xyz')) == tbl)        # True
print(bytes.maketrans(MBA(b'abc'), MBA(b'xyz')) == tbl)   # True

# bytes-like subclass also accepted by bytes.strip / bytearray.strip
print(b'xxabcxx'.strip(MBA(b'x')))                         # b'abc'
print(bytearray(b'xxabcxx').strip(MBA(b'x')))             # bytearray(b'abc')
