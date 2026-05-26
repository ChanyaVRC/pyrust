# Parity tests for bytes.translate() — keyword `delete=` argument forwarding.
# CPython signature: bytes.translate(table, /, delete=b'')

# delete= as keyword argument
print(b'hello world'.translate(None, delete=b'lo'))
print(b'abcabc'.translate(None, delete=b'a'))

# delete= as positional argument (must still work)
print(b'hello world'.translate(None, b'lo'))
print(b'abcabc'.translate(None, b'a'))

# No delete argument at all
print(b'hello'.translate(None))

# Unknown keyword raises TypeError
try:
    b'hello'.translate(None, unknown=b'x')
except TypeError as e:
    print(type(e).__name__)

# delete= provided both positionally and as keyword raises TypeError
try:
    b'hello'.translate(None, b'l', delete=b'o')
except TypeError as e:
    print(type(e).__name__)
