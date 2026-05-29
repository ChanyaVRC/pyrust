# Parity fixture for bytes methods: swapcase, isascii, istitle, partition,
# rpartition, fromhex — closes #1030, #1029, #1009.

# --- swapcase ---
print(b'Hello World'.swapcase())      # b'hELLO wORLD'
print(b'hello'.swapcase())            # b'HELLO'
print(b'HELLO'.swapcase())            # b'hello'
print(b''.swapcase())                 # b''
print(b'123!@#'.swapcase())           # non-alpha unchanged
print(b'Hello World 123'.swapcase())  # mixed

# --- isascii ---
print(b'hello'.isascii())    # True
print(b''.isascii())         # empty -> True
print(b'\x7f'.isascii())     # 127 is ASCII
print(b'\x80'.isascii())     # 128 is not ASCII
print(b'\xff'.isascii())     # 255 is not ASCII
print(b'abc\xff'.isascii())  # any non-ASCII -> False

# --- istitle ---
print(b'Hello World'.istitle())    # True
print(b'hello world'.istitle())    # False (first word lowercase)
print(b'Hello world'.istitle())    # False (second word lowercase)
print(b'hello World'.istitle())    # False (first word lowercase)
print(b'HELLO'.istitle())          # False (all-uppercase)
print(b'Hello'.istitle())          # True
print(b''.istitle())               # False (empty)
print(b'Hello123'.istitle())       # True (digits are non-alpha separators)
print(b'Hello 123'.istitle())      # True
print(b'1Hello'.istitle())         # True (digit then alpha word)
print(b'Hello1World'.istitle())    # True (digit between words)

# --- partition ---
print(b'Hello World'.partition(b' '))        # (b'Hello', b' ', b'World')
print(b'Hello World'.partition(b'x'))        # not found
print(b'hello world hello'.partition(b' '))  # first occurrence
print(b''.partition(b'x'))                   # empty bytes
print(b'aXXbXXc'.partition(b'XX'))           # multi-byte sep

# partition error paths
try:
    b'hello'.partition(b'')
except ValueError as e:
    print('ValueError:', e)
try:
    b'hello'.partition('x')
except TypeError as e:
    print('TypeError:', e)

# --- rpartition ---
print(b'Hello World'.rpartition(b' '))        # (b'Hello', b' ', b'World')
print(b'Hello World'.rpartition(b'x'))        # not found -> (b'', b'', original)
print(b'hello world hello'.rpartition(b' '))  # last occurrence
print(b''.rpartition(b'x'))                   # empty bytes
print(b'aXXbXXc'.rpartition(b'XX'))           # multi-byte sep right

# rpartition error paths
try:
    b'hello'.rpartition(b'')
except ValueError as e:
    print('ValueError:', e)

# --- fromhex ---
print(bytes.fromhex('68656c6c6f'))        # b'hello'
print(bytes.fromhex('68 65 6c 6c 6f'))   # spaces allowed
print(bytes.fromhex('deadbeef'))          # b'\xde\xad\xbe\xef'
print(bytes.fromhex(''))                  # empty
print(bytes.fromhex('48'))               # single byte
print(bytes.fromhex('68\t65'))           # tab whitespace
print(bytes.fromhex('DEADBEEF'))         # uppercase hex

# fromhex accessible on instance
print(b''.fromhex('48656c6c6f'))

# fromhex error paths
try:
    bytes.fromhex('xyz')
except ValueError as e:
    print('ValueError:', e)
try:
    bytes.fromhex('6')
except ValueError as e:
    print('ValueError:', e)
try:
    bytes.fromhex('6g')
except ValueError as e:
    print('ValueError:', e)
try:
    bytes.fromhex(123)
except TypeError as e:
    print('TypeError:', e)
try:
    bytes.fromhex()
except TypeError as e:
    print('TypeError:', e)
try:
    bytes.fromhex('48', 'extra')
except TypeError as e:
    print('TypeError:', e)
