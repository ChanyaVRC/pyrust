# partition
try:
    b'abc'.partition(b'b', 1)
    print('WRONG')
except TypeError as e:
    print('ok', str(e))

try:
    b'abc'.partition()
    print('WRONG')
except TypeError as e:
    print('ok', str(e))

# rpartition
try:
    b'abc'.rpartition(b'b', 1)
    print('WRONG')
except TypeError as e:
    print('ok', str(e))

try:
    b'abc'.rpartition()
    print('WRONG')
except TypeError as e:
    print('ok', str(e))

# bytearray shares the same defect
try:
    bytearray(b'abc').partition(b'b', 1)
    print('WRONG')
except TypeError as e:
    print('ok', str(e))

try:
    bytearray(b'abc').rpartition()
    print('WRONG')
except TypeError as e:
    print('ok', str(e))

# Normal call still works
print(b'abc'.partition(b'b'))    # (b'a', b'b', b'c')
print(b'abc'.rpartition(b'b'))   # (b'a', b'b', b'c')
print(bytearray(b'abc').partition(b'b'))   # (bytearray(b'a'), bytearray(b'b'), bytearray(b'c'))
