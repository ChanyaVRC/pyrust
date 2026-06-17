sep = bytearray(b'-')

# Lazy iterators: generator expression, map, filter (#2538).
print(sep.join(x for x in [b'\x01', b'\x02']))
print(sep.join(map(bytes, [[65], [66]])))
print(sep.join(filter(None, [b'a', b'b'])))


# User-defined __iter__ object.
class It:
    def __iter__(self):
        return iter([b'x', b'y', b'z'])


print(sep.join(It()))

# List / tuple fast path still works.
print(sep.join([b'1', b'2']))
print(sep.join((b'1', b'2')))

# Empty, single-element.
print(sep.join(x for x in []))
print(sep.join([b'only']))

# bytearray elements (coerced to bytes value).
print(sep.join([bytearray(b'p'), bytearray(b'q')]))
print(sep.join(bytearray(x) for x in [[112], [113]]))


# bytearray subclass receiver with a lazy iterable.
class BA(bytearray):
    pass


print(BA(b'+').join(x for x in [b'a', b'b']))

# Error paths match CPython class + wording.
try:
    sep.join(5)
except TypeError as e:
    print("TypeError:", e)

try:
    sep.join(x for x in [b'a', 'str'])
except TypeError as e:
    print("TypeError:", e)

try:
    sep.join(x for x in [b'a', 5])
except TypeError as e:
    print("TypeError:", e)
