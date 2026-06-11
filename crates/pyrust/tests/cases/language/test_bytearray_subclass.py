"""
A `bytearray` subclass instance must inherit the full bytearray behaviour
(iteration, repr/str/format, len, indexing, slicing, mutation, methods,
comparison, equality, membership), matching CPython 3.12 (issue #2324).

Also covers the sibling `set`/`frozenset` f-string `{x}` (str) gap surfaced
alongside it: `f"{x}"` must equal `str(x)` for subclasses whose `__str__`
prefixes the class name.
"""


class BA(bytearray):
    pass


b = BA(b"Hello")

# iteration yields ints (the original bug: 'bytearray.__iter__ not in registry')
print(list(b))
print([x for x in b])
print(tuple(b))
print(sorted(set(b)))
print(sum(x for x in BA(b"\x01\x02\x03")))
for x in b:
    pass
print(list(reversed(BA(b"abc"))))

# repr / str / f-string  -> class-name-prefixed `BA(b'...')`
print(repr(b))
print(str(b))
print(f"{b}")
print(f"{b!r}")
print(repr(BA(b"")))
print(str(BA(b"")))

# len / indexing / slicing
print(len(b))
print(b[0], b[-1])
print(b[1:3])
print(type(b[1:3]).__name__)
print(b[::-1])

# membership
print(72 in b, 200 in b, b"He" in b)

# read methods (return plain bytearray, not the subclass)
print(b.upper())
print(b.lower())
print(b.find(b"ll"))
print(b.count(b"l"))
print(b.split(b"l"))
print(b.hex())
print(b.replace(b"l", b"L"))
print(type(b.upper()).__name__)

# mutation methods
m = BA(b"Hello")
m.append(33)
print(list(m))
m.extend(b"!!")
print(bytes(m))
m.insert(0, 60)
print(m[0])
m.pop()
print(bytes(m))
m[0] = 62
print(m[0])
del m[0]
print(bytes(m))
m.reverse()
print(bytes(m))
m[1:3] = b"XY"
print(bytes(m))
m.clear()
print(bytes(m), len(m))

# bytearray(int) form on the subclass
print(bytes(BA(3)))

# decode
print(BA(b"hi").decode())

# bool
print(bool(BA(b"")), bool(BA(b"x")))

# concatenation / repeat (produce a plain bytearray)
print(bytes(BA(b"ab") + b"cd"))
print(bytes(BA(b"ab") * 2))
print(type(BA(b"ab") + b"cd").__name__)

# comparison + equality
print(BA(b"ab") == bytearray(b"ab"))
print(BA(b"ab") == b"ab")
print(BA(b"ab") != b"xy")
print(BA(b"ab") < BA(b"ac"))
print(BA(b"ac") > BA(b"ab"))
print(BA(b"ab") <= BA(b"ab"))
print(sorted([BA(b"c"), BA(b"a"), BA(b"b")]))

# isinstance / type
print(isinstance(b, bytearray), isinstance(b, BA))
print(BA.__mro__[1].__name__)

# join
print(bytes(BA(b",").join([b"a", b"b", b"c"])))

# nested in a container repr (element __repr__ is dispatched)
print([BA(b"xy"), BA(b"z")])


# A user override still wins over the inherited builtin behaviour.
class BAOverride(bytearray):
    def __iter__(self):
        yield 999

    def __repr__(self):
        return "custom-repr"

    def __str__(self):
        return "custom-str"


o = BAOverride(b"abc")
print(list(o))
print(repr(o))
print(str(o))
print(f"{o}")
print(f"{o!r}")


# bytearray is unhashable; the subclass inherits that — not only via the direct
# `hash()` call but as a dict key / set element too (the insertion paths must
# reject it with the subclass name, matching CPython).
try:
    hash(BA(b"a"))
except TypeError as e:
    print("TypeError:", e)
try:
    {BA(b"a")}
except TypeError as e:
    print("TypeError:", e)
try:
    {BA(b"a"): 1}
except TypeError as e:
    print("TypeError:", e)
_s = set()
try:
    _s.add(BA(b"a"))
except TypeError as e:
    print("TypeError:", e)


# Sibling: set / frozenset subclass `f"{x}"` must equal `str(x)`
# (class-name-prefixed), not the bare backing repr.
class S(set):
    pass


class FS(frozenset):
    pass


print(f"{S({1})}", str(S({1})))
print(f"{FS({1})}", str(FS({1})))
print(f"{S(set())}", str(S(set())))
