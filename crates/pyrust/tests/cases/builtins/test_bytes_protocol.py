# Tests for bytes() calling __bytes__ on user-defined objects (issue #1162).

# Basic: __bytes__ is called and its return value is used.
class MyData:
    def __bytes__(self):
        return b'\x01\x02\x03'

print(bytes(MyData()))

# Priority: __bytes__ takes precedence over __iter__ when both are defined.
class Ambiguous:
    def __bytes__(self):
        return b'from_bytes'
    def __iter__(self):
        return iter([1, 2, 3])

print(bytes(Ambiguous()))

# Fallback: when __bytes__ is absent, iterable path still works.
class IterOnly:
    def __iter__(self):
        return iter([65, 66, 67])

print(bytes(IterOnly()))

# Error: __bytes__ returning a non-bytes value raises TypeError.
class BadReturn:
    def __bytes__(self):
        return 42

try:
    bytes(BadReturn())
except TypeError as e:
    print(e)

# Error: float return from __bytes__ also raises TypeError.
class BadReturnFloat:
    def __bytes__(self):
        return 3.14

try:
    bytes(BadReturnFloat())
except TypeError as e:
    print(e)

# Bools are ints: True==1, False==0; must be accepted in the iterable path.
class BoolIter:
    def __iter__(self):
        return iter([True, False, True])

print(bytes(BoolIter()))

# Inherited __bytes__ via MRO.
class BytesBase:
    def __bytes__(self):
        return b'inherited'

class BytesChild(BytesBase):
    pass

print(bytes(BytesChild()))

# Regression: built-in forms of bytes() must still work.
print(bytes())
print(bytes(5))
print(bytes([65, 66]))
print(bytes(b"abc"))
print(bytes("hi", "utf-8"))
