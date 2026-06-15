"""Issue #2433: object-inherited and primitive-owned dunders report the correct
CPython 3.12 descriptor class (`method_descriptor` / `wrapper_descriptor`) and
repr, not the generic `builtin_function_or_method` / `<built-in function ...>`.
"""

import builtins


def show(obj, name):
    attr = getattr(obj, name)
    print(f"{obj.__name__}.{name}", type(attr).__name__, repr(attr))


# object-inherited method_descriptors.
for m in ["__reduce__", "__reduce_ex__", "__sizeof__", "__dir__", "__format__"]:
    show(object, m)

# object-inherited slot wrappers (wrapper_descriptor).
for m in [
    "__delattr__",
    "__setattr__",
    "__getattribute__",
    "__init__",
    "__str__",
    "__hash__",
    "__repr__",
    "__eq__",
    "__ne__",
    "__lt__",
    "__le__",
    "__gt__",
    "__ge__",
]:
    show(object, m)

# str/int/float __format__ is a method_descriptor; __hash__/__repr__/__str__ are
# slot wrappers owned by the type.
show(str, "__format__")
show(int, "__format__")
show(float, "__format__")
show(str, "__hash__")
show(str, "__repr__")
show(str, "__str__")
show(int, "__hash__")
show(int, "__repr__")

# Arithmetic/comparison slot wrappers on int.
for m in [
    "__add__",
    "__sub__",
    "__mul__",
    "__floordiv__",
    "__truediv__",
    "__mod__",
    "__pow__",
    "__bool__",
    "__float__",
    "__int__",
    "__eq__",
    "__ne__",
    "__lt__",
    "__le__",
    "__gt__",
    "__ge__",
]:
    show(int, m)

# Primitive-owned comparison slot wrappers — the owner must be the type itself,
# not the inherited `object` slot.
for t in [list, tuple, dict, set, frozenset, bytes, bytearray, float, complex]:
    for m in ["__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__"]:
        show(t, m)

# bool inherits int's numeric/bool slots (owner is int).
print(type(bool.__float__).__name__, repr(bool.__float__))
print(type(bool.__bool__).__name__, repr(bool.__bool__))
print(type(bool.__add__).__name__, repr(bool.__add__))

# The descriptors are callable and behave as CPython does.
print(int.__float__(5), int.__int__(5), int.__bool__(0))
print((5).__float__(), (5).__int__(), (5).__bool__())
print(list.__eq__([1, 2], [1, 2]), list.__lt__([1], [1, 2]))
print(str.__format__("hi", ">5"))
print(int.__format__(255, "x"))
print(object.__sizeof__(object()) >= 0)
