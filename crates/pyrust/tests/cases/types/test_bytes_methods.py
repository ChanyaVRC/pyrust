print(b'hello'.hex())            # 68656c6c6f
print(b'\xff'.hex())             # ff
print(b'\xff\xfe'.hex(':'))      # ff:fe
print(b'hello'.decode())         # hello
print(b'hello'.decode('utf-8'))  # hello
print(b'hello'.startswith(b'hel'))  # True
print(b'hello'.startswith(b'xyz'))  # False
print(b'hello'.endswith(b'lo'))     # True
print(b'hello'.endswith(b'xyz'))    # False
print(b'hello'.find(b'ell'))     # 1
print(b'hello'.find(b'xyz'))     # -1
print(b'abab'.count(b'ab'))      # 2
print(b'hello'.count(b'l'))      # 2
print(b'hello'.upper())          # b'HELLO'
print(b'HELLO'.lower())          # b'hello'

# hasattr
print(hasattr(b'hello', 'hex'))      # True
print(hasattr(b'hello', 'decode'))   # True
print(hasattr(b'hello', 'nomethod')) # False

# find with start/end
print(b'hello'.find(b'l', 3))    # 3
print(b'hello'.find(b'l', 0, 3)) # 2

# count with slice
print(b'hello'.count(b'l', 2, 5)) # 2

# startswith with empty bytes
print(b'hello'.startswith(b''))   # True

# decode latin-1
print(b'\xff'.decode('latin-1'))  # ÿ

# decode ascii
print(b'hello'.decode('ascii'))   # hello

# upper/lower type
print(type(b'hello'.upper()))     # <class 'bytes'>
print(type(b'HELLO'.lower()))     # <class 'bytes'>

# Error: invalid utf-8
try:
    b'\xff'.decode('utf-8')
except UnicodeDecodeError as e:
    print("UnicodeDecodeError caught")

# Error: unknown encoding
try:
    b'hello'.decode('xyz')
except LookupError as e:
    print("LookupError caught")

# Error: startswith non-bytes
try:
    b'hello'.startswith('hel')
except TypeError as e:
    print("TypeError caught")

# Error: endswith non-bytes
try:
    b'hello'.endswith('lo')
except TypeError as e:
    print("TypeError caught")
