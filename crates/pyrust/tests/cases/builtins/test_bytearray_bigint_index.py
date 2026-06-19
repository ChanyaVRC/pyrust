BIG = 1 << 70
ba = bytearray(b'abc')

# getitem
try:
    _ = ba[BIG]
    print('WRONG')
except IndexError as e:
    print('ok', str(e) == "cannot fit 'int' into an index-sized integer")

# negative big
try:
    _ = ba[-BIG]
    print('WRONG')
except IndexError as e:
    print('ok', str(e) == "cannot fit 'int' into an index-sized integer")

# setitem
try:
    ba[BIG] = 9
    print('WRONG')
except IndexError as e:
    print('ok')

# delitem
try:
    del ba[BIG]
    print('WRONG')
except IndexError as e:
    print('ok')

# Normal usage still works
print(ba[0])    # 97
ba[0] = 88
print(ba[0])    # 88
