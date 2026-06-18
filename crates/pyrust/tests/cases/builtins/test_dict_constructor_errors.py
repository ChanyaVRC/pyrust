# dict() constructor error wording for non-iterable subclass args (issue #2563)
# CPython 3.12: a non-iterable builtin subclass reports the *subclass* name,
# not the backing primitive's type name.

# --- error cases ---


# int subclass: not iterable -> "'C' object is not iterable"
class C(int):
    pass


try:
    dict(C(5))
except TypeError as e:
    print(repr(str(e)))


# float subclass: same wording with the subclass name
class F(float):
    pass


try:
    dict(F(1.5))
except TypeError as e:
    print(repr(str(e)))


# plain non-iterable non-subclass: backing primitive's own type name
try:
    dict(42)
except TypeError as e:
    print(repr(str(e)))

try:
    dict(3.5)
except TypeError as e:
    print(repr(str(e)))


# iterable of pairs but wrong element length: ValueError, not TypeError
try:
    dict([(1,)])
except ValueError as e:
    print(type(e).__name__)

# --- success cases ---


# list subclass: iterable of (key, value) pairs still works
class D(list):
    pass


print(dict(D([(1, 2), (3, 4)])))

# str subclass: iterable backing still iterates as a sequence of pairs-ish
# strings; here a list of 2-char strings becomes key/value pairs.
print(dict(["ab", "cd"]))

# plain mapping
print(dict({"a": 1, "b": 2}))

# plain iterable of pairs
print(dict([(1, 2), (3, 4)]))

# dict subclass from collections still reads via the mapping protocol
import collections

print(dict(collections.Counter("aabbbc")))
