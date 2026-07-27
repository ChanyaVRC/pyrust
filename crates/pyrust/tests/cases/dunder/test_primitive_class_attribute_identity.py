"""Other primitive class adapters use the same stable identity boundary."""

import builtins


def signatures(cls, names):
    result = []
    for name in names:
        try:
            descriptor = getattr(cls, name)
        except AttributeError:
            result.append((name, "missing"))
        else:
            result.append((name, type(descriptor).__name__, repr(descriptor)))
    return result


INT_ATTRS = ["real", "imag", "numerator", "denominator", "conjugate"]
FLOAT_ATTRS = ["real", "imag", "conjugate"]

# Direct builtins and genuine subclasses resolve to the canonical descriptor
# owner. bool inherits the int descriptors.
print("int", signatures(int, INT_ATTRS))
print("bool", signatures(bool, INT_ATTRS))
print("int subclass", signatures(type("I", (int,), {}), INT_ATTRS))
print("float", signatures(float, FLOAT_ATTRS))
print("float subclass", signatures(type("F", (float,), {}), FLOAT_ATTRS))

# A user class that merely borrows the visible name has no numeric descriptor.
print("int spoof", signatures(type("int", (), {}), INT_ATTRS))
print("float spoof", signatures(type("float", (), {}), FLOAT_ATTRS))

# dict.fromkeys has a subclass-binding adapter. It must have the same identity
# rule: genuine dict subclasses inherit it, a user class named dict does not.
print("dict subclass fromkeys", hasattr(type("D", (dict,), {}), "fromkeys"))
print("dict spoof fromkeys", hasattr(type("dict", (), {}), "fromkeys"))
