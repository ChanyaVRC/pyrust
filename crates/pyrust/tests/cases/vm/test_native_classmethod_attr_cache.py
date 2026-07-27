# Native classmethod attribute-cache parity.
#
# The optimized path is descriptor-category based: it must preserve binding,
# owner validation, fresh bound-object identity, MRO lookup, and invalidation
# without recognizing `from_bytes` (or any other Python-visible method name).


def invoke(cls):
    return cls.from_bytes(b"\x01", "big")


def read(cls):
    return cls.from_bytes


# Exact provider and inherited-subclass binding.
print("int", [invoke(int) for _ in range(5)])


class HotInt(int):
    pass


print("subclass", [invoke(HotInt) for _ in range(5)])
first = read(HotInt)
second = read(HotInt)
print(
    "fresh",
    first is second,
    first == second,
    first.__self__ is HotInt,
    first.__name__,
)

# A mutation of the target class invalidates both CallMethod and GetAttr sites.
HotInt.from_bytes = classmethod(lambda cls, value, order: 41)
print("target-patched", invoke(HotInt), read(HotInt).__self__ is HotInt)
del HotInt.from_bytes
print("target-restored", invoke(HotInt), read(HotInt).__self__ is HotInt)


# An inherited lookup is invalidated when an ancestor changes.
class MiddleInt(int):
    pass


class LeafInt(MiddleInt):
    pass


print("mro-native", invoke(LeafInt))
MiddleInt.from_bytes = classmethod(lambda cls, value, order: 73)
print("mro-patched", invoke(LeafInt))
del MiddleInt.from_bytes
print("mro-restored", invoke(LeafInt))


# The same opcode site may see unrelated class objects. The native descriptor
# must not bleed into a normal Python classmethod.
class Regular:
    @classmethod
    def from_bytes(cls, value, order):
        return cls.__name__ + ":" + order


print("regular", invoke(Regular))
Regular.from_bytes = classmethod(lambda cls, value, order: "changed")
print("regular-patched", invoke(Regular))


# A metaclass data descriptor has precedence over the class MRO and is never
# bypassed by the native descriptor cache.
class Meta(type):
    @property
    def from_bytes(cls):
        return lambda value, order: 99


class MetaInt(int, metaclass=Meta):
    pass


print("metaclass-data", invoke(MetaInt))


# Provider ownership remains attached to the descriptor when aliased.
raw = vars(int)["from_bytes"]


class WrongOwner:
    from_bytes = raw


try:
    invoke(WrongOwner)
except TypeError as exc:
    print("wrong-owner", type(exc).__name__, str(exc))
