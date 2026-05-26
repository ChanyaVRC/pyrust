# Tests for bytes.maketrans() and bytes.translate() — issue #1250

# --- maketrans: basic usage ---
table = bytes.maketrans(b'abc', b'xyz')
print(b'abcdef'.translate(table))          # b'xyzdef'

# Instance-method form (staticmethod: ignores the receiver)
print(b'hello'.maketrans(b'abc', b'xyz') == table)  # True

# Empty mapping: identity table
identity = bytes.maketrans(b'', b'')
print(b'hello'.translate(identity))        # b'hello'

# --- translate: None table (delete-only) ---
print(b'hello world'.translate(None, b'aeiou'))     # b'hll wrld'
print(b'hello world'.translate(None, delete=b'aeiou'))  # b'hll wrld'

# --- translate: table + delete ---
print(b'hello'.translate(bytes.maketrans(b'h', b'H'), b'l'))  # b'Heo'

# --- translate: table only, no delete ---
print(b'abc'.translate(table))             # b'xyz'

# --- translate: empty input ---
print(b''.translate(table))               # b''

# --- translate: delete all ---
print(b'hello'.translate(None, b'hello'))  # b''

# --- maketrans: ValueError when lengths differ ---
try:
    bytes.maketrans(b'abc', b'ab')
except ValueError as e:
    print(e)   # maketrans arguments must have same length

# --- translate: ValueError when table is not 256 bytes ---
try:
    b'hello'.translate(b'x' * 200)
except ValueError as e:
    print(e)   # translation table must be 256 characters long

try:
    b'hello'.translate(b'x' * 257)
except ValueError as e:
    print(e)   # translation table must be 256 characters long

# --- translate: TypeError for wrong table type ---
try:
    b'hello'.translate(42)
except TypeError as e:
    print(e)   # a bytes-like object is required, not 'int'

# --- translate: TypeError for wrong delete type ---
try:
    b'hello'.translate(None, 'aeiou')
except TypeError as e:
    print(e)   # a bytes-like object is required, not 'str'

# --- translate: no arguments ---
try:
    b'hello'.translate()
except TypeError as e:
    print(e)   # translate() takes at least 1 positional argument (0 given)

# --- translate: too many arguments ---
try:
    b'hello'.translate(None, b'a', b'b')
except TypeError as e:
    print(e)   # translate() takes at most 2 arguments (3 given)

# --- translate: invalid keyword argument ---
try:
    b'hello'.translate(None, foo=b'a')
except TypeError as e:
    print(e)   # 'foo' is an invalid keyword argument for translate()

# --- maketrans: arity errors ---
try:
    bytes.maketrans(b'a')
except TypeError as e:
    print(e)   # maketrans expected 2 arguments, got 1

try:
    bytes.maketrans(b'a', b'b', b'c')
except TypeError as e:
    print(e)   # maketrans expected 2 arguments, got 3

# --- maketrans: non-bytes args ---
try:
    bytes.maketrans('abc', b'xyz')
except TypeError as e:
    print(e)   # a bytes-like object is required, not 'str'

# --- dir/hasattr ---
print('maketrans' in dir(b''))             # True
print(hasattr(b'', 'maketrans'))           # True
print(hasattr(bytes, 'maketrans'))         # True
