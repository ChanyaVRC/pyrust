"""Primitive slot owners come from builtin identity, never class names.

The compact signatures below cover every primitive affected by the synthetic
object/rich-comparison descriptor path.  Each entry is
``(descriptor type, descriptor repr)`` (or ``("NoneType", "None")`` for the
four unhashable primitives), so both descriptor kind and displayed owner are
checked byte-for-byte against CPython 3.12.
"""

import builtins


PRIMITIVES = [
    ("bool", builtins.bool),
    ("int", builtins.int),
    ("float", builtins.float),
    ("complex", builtins.complex),
    ("str", builtins.str),
    ("bytes", builtins.bytes),
    ("bytearray", builtins.bytearray),
    ("tuple", builtins.tuple),
    ("frozenset", builtins.frozenset),
    ("list", builtins.list),
    ("dict", builtins.dict),
    ("set", builtins.set),
]

SLOTS = [
    "__hash__",
    "__repr__",
    "__str__",
    "__format__",
    "__eq__",
    "__ne__",
    "__lt__",
    "__le__",
    "__gt__",
    "__ge__",
]


def signatures(cls):
    result = []
    for slot in SLOTS:
        descriptor = getattr(cls, slot)
        result.append((type(descriptor).__name__, repr(descriptor)))
    return result


# Direct builtins pin the complete ownership table, including bool's inherited
# int slots, bytearray-owned repr/str, and complex-owned format.
for type_name, primitive in PRIMITIVES:
    print("builtin", type_name, signatures(primitive))


# Regression: a plain user class with a builtin-looking __name__ inherits every
# one of these descriptors from object.  It must not acquire primitive slots.
for type_name, _primitive in PRIMITIVES:
    spoof = type(type_name, (), {})
    print("spoof", type_name, signatures(spoof))


# Genuine builtin subclasses retain the builtin's descriptor owners.  bool is
# intentionally absent because CPython marks it non-subclassable.
for type_name, primitive in PRIMITIVES:
    if primitive is bool:
        continue
    subclass = type("OwnerProbe", (primitive,), {})
    print("subclass", type_name, signatures(subclass))


# Newly-covered owner rows remain callable through their descriptors.
print("complex format", complex.__format__(1 + 2j, ".2f"))
print("bytearray repr", bytearray.__repr__(bytearray(b"ab")))
print("bytearray str", bytearray.__str__(bytearray(b"ab")))
