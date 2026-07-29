# Issue #2770: unbound `bytearray.<method>(ba, ...)` calls work like the bound
# form (and like `bytes`/`str` unbound calls), instead of raising
# `TypeError: 'method_descriptor' object is not callable`.

ba = bytearray(b'hello world')

# Read methods returning a new bytearray / value.
print(bytearray.replace(ba, b'hello', b'goodbye'))
print(bytearray.upper(ba))
print(bytearray.lower(bytearray(b'HELLO')))
print(bytearray.title(ba))
print(bytearray.count(ba, b'l'))
print(bytearray.find(ba, b'world'))
print(bytearray.split(ba))
print(bytearray.split(bytearray(b'a,b,c'), b',', maxsplit=1))
print(bytearray.join(bytearray(b'-'), [b'a', b'b', b'c']))
print(bytearray.hex(bytearray(b'AB')))
print(bytearray.decode(bytearray(b'hi')))
print(bytearray.startswith(bytearray(b'hello'), b'he'))
print(bytearray.copy(bytearray(b'xy')))

# Classmethod via the type.
print(bytearray.fromhex('48 49'))

# Iterator dunder.
print(list(bytearray.__iter__(bytearray(b'abc'))))

# Mutating methods operate on the passed receiver (shared backing).
m = bytearray(b'abc')
bytearray.append(m, 122)
print(m)
bytearray.extend(m, b'de')
print(m)
bytearray.reverse(m)
print(m)
bytearray.insert(m, 0, 120)
print(m)
print(bytearray.pop(m))
print(m)
bytearray.clear(m)
print(m)

# Receiver-type guard (method_descriptor wording).
try:
    bytearray.replace("hello", b'h', b'x')
except TypeError as e:
    print(e)
try:
    bytearray.upper(b'hello')
except TypeError as e:
    print(e)

# Missing receiver argument.
try:
    bytearray.replace()
except TypeError as e:
    print(e)

# Keyword arguments rejected by no-kwarg method_descriptors.
try:
    bytearray.find(ba, b'h', start=0)
except TypeError as e:
    print(e)

# Subclass receiver is accepted (parity with bytes).
class MyBA(bytearray):
    pass

print(bytearray.upper(MyBA(b'hi')))
