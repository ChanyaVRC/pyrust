# Native callable categories are declared by the built-in class provider.
# Generic attribute lookup must not infer them from dotted dispatch keys.

class_methods = (
    (int, "from_bytes", 0),
    (float, "fromhex", 0.0),
    (bytes, "fromhex", b""),
    (bytearray, "fromhex", bytearray()),
    (dict, "fromkeys", {}),
    (object, "__init_subclass__", object()),
    (object, "__subclasshook__", object()),
    (type, "__prepare__", type),
)

for owner, name, instance in class_methods:
    raw = vars(owner)[name]
    on_class = getattr(owner, name)
    on_instance = getattr(instance, name)
    print(
        owner.__name__,
        name,
        type(raw).__name__,
        hasattr(raw, "__func__"),
        raw.__objclass__ is owner,
        raw.__name__,
        raw.__qualname__,
    )
    print(raw == vars(owner)[name], hash(raw) == hash(vars(owner)[name]))
    print(
        type(on_class).__name__,
        on_class.__self__ is owner,
        type(on_instance).__name__,
        on_instance.__self__ is owner,
    )
    repeated = getattr(owner, name)
    print(
        on_class is repeated,
        on_class == repeated,
        hash(on_class) == hash(repeated),
        repr(on_class).startswith("<built-in method " + name + " of type object at 0x")
        and repr(on_class).endswith(">"),
        hasattr(on_class, "__call__"),
    )

static_methods = (
    (bytes, "maketrans", b""),
    (bytearray, "maketrans", bytearray()),
    (str, "maketrans", ""),
)

for owner, name, instance in static_methods:
    raw = vars(owner)[name]
    on_class = getattr(owner, name)
    on_instance = getattr(instance, name)
    print(
        owner.__name__,
        name,
        type(raw).__name__,
        raw.__func__ is on_class,
        on_class is on_instance,
        type(on_class).__name__,
        on_class.__self__ is None,
        on_class.__qualname__,
    )
    print(raw == vars(owner)[name], hash(raw) == hash(vars(owner)[name]))
    repeated = getattr(owner, name)
    print(
        on_class is repeated,
        on_class == repeated,
        hash(on_class) == hash(repeated),
        repr(on_class).startswith("<built-in method " + name + " of type object at 0x")
        and repr(on_class).endswith(">"),
        hasattr(on_class, "__call__"),
    )

# Binding metadata must not alter the existing dispatch behavior.
print(int.from_bytes(b"\x01", "big") == 1)
print(float.fromhex("0x1p+1") == 2.0)
print(bytes.fromhex("4142") == b"AB")
print(bytearray.fromhex("4142") == bytearray(b"AB"))
print(dict.fromkeys([1, 2], 9) == {1: 9, 2: 9})
print(bytes.maketrans(b"a", b"b")[97] == 98)
print(bytearray.maketrans(b"a", b"b")[97] == 98)
print(str.maketrans("a", "b")[97] == 98)
print(int.from_bytes.__call__(b"\x01", "big") == 1)
print(bytes.maketrans.__call__(b"a", b"b")[97] == 98)


class MyInt(int):
    pass


class MyDict(dict):
    pass


# Inherited classmethod descriptors bind the concrete subclass, whether
# accessed through the class or one of its instances.
print(MyInt.from_bytes.__self__ is MyInt, MyInt(0).from_bytes.__self__ is MyInt)
print(MyDict.fromkeys.__self__ is MyDict, MyDict().fromkeys.__self__ is MyDict)


# The provider-owned descriptor retains its defining class when aliased.
# Binding is valid for subclasses and rejected for unrelated classes.
raw_from_bytes = vars(int)["from_bytes"]


class AliasInt(int):
    aliased = raw_from_bytes


print(
    AliasInt.aliased.__self__ is AliasInt,
    AliasInt(0).aliased.__self__ is AliasInt,
)


class WrongOwner:
    aliased = raw_from_bytes


for read in (
    lambda: WrongOwner.aliased,
    lambda: WrongOwner().aliased,
    lambda: raw_from_bytes.__get__(None, str),
):
    try:
        read()
        print("FAIL: classmethod_descriptor owner check")
    except TypeError as exc:
        print(type(exc).__name__, str(exc))


class WrongPrepare:
    prepare = vars(type)["__prepare__"]


try:
    WrongPrepare.prepare
    print("FAIL: type.__prepare__ owner check")
except TypeError as exc:
    print(type(exc).__name__, str(exc))


class Meta(type):
    prepare = vars(type)["__prepare__"]


print(Meta.prepare.__self__ is Meta)


# `collections.abc` is implemented by the interpreter, but its public surface
# mirrors a Python library module: the hooks remain ordinary `classmethod`
# descriptors and bind to `method`, rather than presenting as native
# `classmethod_descriptor` / `builtin_function_or_method` objects.
from collections.abc import Iterable

abc_raw = vars(Iterable)["__subclasshook__"]
abc_bound = Iterable.__subclasshook__
print(
    type(abc_raw).__name__,
    hasattr(abc_raw, "__func__"),
    hasattr(abc_raw, "__objclass__"),
)
print(
    type(abc_bound).__name__,
    abc_bound.__self__ is Iterable,
    abc_bound.__func__ is abc_raw.__func__,
)
