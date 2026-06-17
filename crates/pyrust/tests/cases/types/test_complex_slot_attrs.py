# Issue #2536: complex slot dunders exposed as type-level attributes.

# Type-level attribute access -> complex-owned slot wrappers.  Every
# complex-owned dunder reprs as `<slot wrapper '<n>' of 'complex' objects>`;
# the inherited `__str__` reprs as object's slot wrapper (checked below).
for _n in (
    "__eq__", "__ne__", "__lt__", "__le__", "__gt__", "__ge__",
    "__hash__", "__repr__", "__bool__",
    "__add__", "__sub__", "__mul__", "__truediv__", "__pow__",
    "__neg__", "__pos__", "__abs__",
    "__radd__", "__rsub__", "__rmul__", "__rtruediv__", "__rpow__",
):
    print(_n, getattr(complex, _n))

# hasattr on the type.
print(hasattr(complex, "__add__"))
print(hasattr(complex, "__abs__"))
print(hasattr(complex, "__bool__"))
print(hasattr(complex, "__hash__"))

# Unbound calls.
print(complex.__add__(1 + 2j, 3 + 4j))
print(complex.__sub__(1 + 2j, 3 + 4j))
print(complex.__mul__(1 + 2j, 3 + 4j))
print(complex.__truediv__(1 + 2j, 2 + 0j))
print(complex.__pow__(1 + 1j, 2 + 0j))
print(complex.__neg__(1 + 2j))
print(complex.__pos__(1 + 2j))
print(complex.__abs__(3 + 4j))
print(complex.__bool__(0j))
print(complex.__bool__(1j))
print(complex.__hash__(1 + 0j))
print(complex.__repr__(1 + 2j))

# Reflected slots.
print(complex.__radd__(1 + 2j, 3 + 4j))
print(complex.__rmul__(1 + 2j, 3 + 4j))

# Rich comparisons: __eq__/__ne__ compute; ordering slots exist but return
# NotImplemented (complex has no ordering).
print(complex.__eq__(1j, 1j))
print(complex.__eq__(1j, 2j))
print(complex.__ne__(1j, 2j))
print(complex.__lt__(1j, 2j))
print(complex.__le__(1j, 2j))

# __str__ is inherited from object (NOT complex-owned), so the type attr
# resolves to object's slot wrapper, but the bound call still works.
print(complex.__str__)
print(complex.__str__(1 + 2j))

# Bound calls keep working (regression guard).
print((1 + 2j).__abs__())
print((1 + 2j).__add__(3 + 4j))
print((1 + 2j).__neg__())
print((1 + 2j).conjugate())

# Subclass resolves the slot via MRO.
class C(complex):
    pass


print(C.__add__)
print(C.__abs__(3 + 4j))

# Unknown attribute still raises AttributeError with the CPython wording.
try:
    (1 + 2j).bogus()
except AttributeError as e:
    print("AttributeError:", e)
