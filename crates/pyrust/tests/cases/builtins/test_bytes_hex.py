# Parity fixture: bytes.hex() must accept a bytes separator (CPython 3.12+).
# Also exercises str separator and the bytes_per_sep argument.

# No separator: plain hex string
print(b'\xde\xad\xbe\xef'.hex())

# str separator
print(b'\xde\xad\xbe\xef'.hex('-'))

# bytes separator — this was the bug: previously raised TypeError
print(b'\xde\xad\xbe\xef'.hex(b'-'))

# bytes separator with positive bytes_per_sep
print(b'\xde\xad\xbe\xef'.hex(b'-', 2))

# str separator with negative bytes_per_sep (groups from the left)
print(b'\xde\xad\xbe\xef'.hex(':', -2))

# bytes separator with negative bytes_per_sep
print(b'\xde\xad\xbe\xef'.hex(b':', -2))

# Empty bytes: always returns ''
print(b''.hex('-'))

# Single byte: no separator inserted
print(b'\xab'.hex('-'))

# Longer input with bytes separator
print(b'hello'.hex(b':'))
print(b'hello'.hex(b':', 2))

# bytes_per_sep=0 means no separator (CPython behaviour)
print(b'\xde\xad\xbe\xef'.hex('-', 0))

# --- Error cases ---

# bytes separator longer than 1 byte → ValueError
try:
    b'hello'.hex(b'::')
except ValueError as e:
    print("ValueError:", e)

# empty bytes separator → ValueError
try:
    b'hello'.hex(b'')
except ValueError as e:
    print("ValueError:", e)

# non-ASCII byte separator → ValueError
try:
    b'hello'.hex(b'\xff')
except ValueError as e:
    print("ValueError:", e)

# str separator with non-ASCII character → ValueError
try:
    b'hello'.hex('\xff')
except ValueError as e:
    print("ValueError:", e)

# str separator longer than 1 char → ValueError
try:
    b'hello'.hex('--')
except ValueError as e:
    print("ValueError:", e)

# empty str separator → ValueError
try:
    b'hello'.hex('')
except ValueError as e:
    print("ValueError:", e)
