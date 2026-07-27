# Regression coverage for the exact-bytes fused method dispatcher.
#
# A source-level ``obj.method(...)`` reaches the CallMethod family, while
# extracting ``obj.method`` first materialises a bound callable.  Both paths
# must agree, except that fromhex/maketrans deliberately leave the fused
# instance-method table and use their class/static descriptors.

data = b"Ab caB"

# Ordinary instance methods: positional and keyword-bearing calls.
print(data.upper())
print(data.lower())
print(data.replace(b"b", b"!"))
print(data.partition(b" "))
print(b"aabbcc".hex(sep=b":", bytes_per_sep=2))
print(b"\x41\x42".decode(encoding="ascii"))

upper = data.upper
replace = data.replace
hex_method = b"aabbcc".hex
decode = b"\x41\x42".decode
print(upper())
print(replace(b"b", b"!"))
print(hex_method(sep=b":", bytes_per_sep=2))
print(decode(encoding="ascii"))

# These two names are accessible through an instance, but retain descriptor
# semantics: fromhex is a classmethod and maketrans is a staticmethod.
print(b"ignored".fromhex("41 42 43"))
print(b"ignored".maketrans(b"ab", b"xy")[:4])

fromhex = b"ignored".fromhex
maketrans = b"ignored".maketrans
print(fromhex.__self__ is bytes, fromhex.__name__, fromhex.__qualname__)
print(maketrans.__self__ is None, maketrans.__name__, maketrans.__qualname__)
print(fromhex("64 65 66"))
print(maketrans(b"az", b"AZ")[97], maketrans(b"az", b"AZ")[122])

# Container protocol wrappers. __iter__ and __getnewargs__ are owned by the
# bytes method table; the other wrappers use the generic protocol adapter.
print(list(data.__iter__()))
print(data.__len__(), data.__getitem__(2), data.__contains__(b"ca"))
print(data.__getnewargs__())
print(data.__format__(""))

iter_method = data.__iter__
len_method = data.__len__
getitem = data.__getitem__
contains = data.__contains__
getnewargs = data.__getnewargs__
format_method = data.__format__
print(list(iter_method()))
print(len_method(), getitem(2), contains(b"ca"))
print(getnewargs())
print(format_method(""))

# Object-protocol wrappers must remain reachable in both call shapes without
# being mistaken for bytes instance methods.
print(isinstance(data.__sizeof__(), int))
print("upper" in data.__dir__())
print(data.__getstate__())
print(isinstance(data.__reduce_ex__(4), tuple))

sizeof = data.__sizeof__
dir_method = data.__dir__
getstate = data.__getstate__
reduce_ex = data.__reduce_ex__
print(isinstance(sizeof(), int))
print("upper" in dir_method())
print(getstate())
print(isinstance(reduce_ex(4), tuple))

# An unknown method must fall through to ordinary attribute lookup and retain
# AttributeError semantics rather than leaking the bytes dispatcher RuntimeError.
try:
    data.not_a_bytes_method()
except AttributeError as exc:
    print(type(exc).__name__, exc)

try:
    missing = data.not_a_bytes_attribute
except AttributeError as exc:
    print(type(exc).__name__, exc)
