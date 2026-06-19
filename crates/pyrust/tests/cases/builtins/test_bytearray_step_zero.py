ba = bytearray(b'abc')

try:
    _ = ba[::0]
    print('WRONG')
except ValueError as e:
    print('ok', str(e) == "slice step cannot be zero")

try:
    _ = ba[1:2:0]
    print('WRONG')
except ValueError as e:
    print('ok')

# Slice deletion step=0
ba_del = bytearray(b'abc')
try:
    del ba_del[::0]
    print('WRONG')
except ValueError:
    print('ok')

# Slice assignment step=0
ba2 = bytearray(b'abc')
try:
    ba2[::0] = b''
    print('WRONG')
except ValueError:
    print('ok')

# Normal step still works
print(ba[::2])    # bytearray(b'ac')
print(ba[::-1])   # bytearray(b'cba')
print(ba[1:3])    # bytearray(b'bc')
