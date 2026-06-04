# Issues #2150 / #2151: every built-in value exposes the object-protocol
# attributes inherited from `object`.
#
# `obj.__class__ is type(obj)` for all values (#2150); the object-protocol
# dunders (`__doc__`/`__sizeof__`/`__dir__`/`__reduce__`/`__reduce_ex__`/
# `__getstate__`) plus `None.__bool__` are accessible (#2151).
#
# Deterministic members (`__class__`, `__dir__`, `__doc__`) are compared
# exactly; implementation-specific members (`__sizeof__`, `__reduce__`,
# `__reduce_ex__`) are asserted by return TYPE only.

# --- __class__ is type(obj) for every built-in value (#2150) -----------------
print((5).__class__ is int)
print((5).__class__ is type(5))
print("x".__class__ is str)
print([1].__class__ is list)
print({}.__class__ is dict)
print({1}.__class__ is set)
print((1,).__class__ is tuple)
print(b"x".__class__ is bytes)
print(True.__class__ is bool)
print((1.0).__class__ is float)
print((1 + 2j).__class__ is complex)
print(frozenset().__class__ is frozenset)
print(range(3).__class__ is range)
print(None.__class__ is type(None))
print(None.__class__.__name__)
print(int.__class__ is type)
print((lambda: 0).__class__.__name__)

# user instances unchanged
class C:
    pass


print(C().__class__ is C)

# --- hasattr/getattr for __class__ on every value (#2150) -------------------
print(hasattr(5, "__class__"))
print(hasattr(None, "__class__"))
print(getattr(5, "__class__") is int)

# --- object-protocol dunder existence (#2151) -------------------------------
for obj in [5, "x", [1], {}, {1}, (1,), b"x", 1.0, None, frozenset()]:
    print(
        hasattr(obj, "__doc__"),
        hasattr(obj, "__sizeof__"),
        hasattr(obj, "__dir__"),
        hasattr(obj, "__reduce__"),
        hasattr(obj, "__reduce_ex__"),
        hasattr(obj, "__getstate__"),
    )

# --- __doc__ is the type docstring (deterministic) --------------------------
print((5).__doc__ == int.__doc__)
print([1].__doc__ == list.__doc__)
print(None.__doc__ == type(None).__doc__)

# --- __dir__() returns a list whose members match dir() (set-compared, since
# the order of object.__dir__() is implementation-specific) ------------------
print(type([1].__dir__()) is list)
print(set((5).__dir__()) == set(dir(5)))
print(set([1].__dir__()) == set(dir([1])))
print(set(None.__dir__()) == set(dir(None)))

# --- implementation-specific members: TYPE only -----------------------------
# (`__reduce__()` is intentionally not called: CPython raises for unpicklable
# scalars like int, while pyrust does not model copyreg — the exact reduction
# value is impractical to reproduce.  Only existence + the protocol-form return
# type are asserted here.)
print(type((5).__sizeof__()) is int)
print(type([1].__sizeof__()) is int)
print(type(None.__sizeof__()) is int)
print(type((5).__reduce_ex__(2)) is tuple)
print(type([1].__reduce_ex__(2)) is tuple)
print((5).__getstate__())  # None for stateless built-in values

# --- None behaves like other objects (#2151) --------------------------------
print(None.__bool__())
print(hasattr(None, "__bool__"))
print(hasattr(None, "__str__"))
print(hasattr(None, "__eq__"))
print(None == None)
print(bool(None))
print(repr(None))

# --- dir/hasattr consistency for every value --------------------------------
for obj in [5, 1.0, "x", [1], {}, {1}, (1,), b"x", None, frozenset(), range(3)]:
    print(all(hasattr(obj, n) for n in dir(obj)))
