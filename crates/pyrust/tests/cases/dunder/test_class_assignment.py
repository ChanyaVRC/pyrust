# Issue #1957: `obj.__class__ = NewType` re-types the instance.
class X:
    pass


class Y:
    pass


o = X()
o.__class__ = Y
print(type(o).__name__)
print(isinstance(o, Y))
print(isinstance(o, X))
print(o.__class__.__name__)

# Re-typing preserves the instance's existing attributes.
o2 = X()
o2.tag = 7
o2.__class__ = Y
print(o2.tag)

# Round-trip back to the original class.
o2.__class__ = X
print(type(o2).__name__)

# Assigning a non-class raises TypeError with CPython's message.
try:
    o.__class__ = 5
except TypeError as e:
    print(e)

try:
    o.__class__ = "str"
except TypeError as e:
    print(e)

# Re-typing to a built-in immutable class is rejected (CPython parity).
for T in (int, str, list, tuple, dict, object):
    try:
        X().__class__ = T
        print(T.__name__, "unexpected OK")
    except TypeError as e:
        print(T.__name__, e)
