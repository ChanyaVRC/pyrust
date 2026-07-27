"""Primitive providers preserve their declared descriptor categories."""

class_methods = (
    (int, "from_bytes", 0),
    (float, "fromhex", 0.0),
    (bytes, "fromhex", b""),
    (bytearray, "fromhex", bytearray()),
    (dict, "fromkeys", {}),
)

for owner, name, instance in class_methods:
    raw = vars(owner)[name]
    from_class = getattr(owner, name)
    from_instance = getattr(instance, name)
    print(
        owner.__name__,
        name,
        type(raw).__name__,
        raw is vars(owner)[name],
        from_class.__self__ is owner,
        from_instance.__self__ is owner,
        name in dir(owner),
        name in dir(instance),
    )

static_methods = (
    (bytes, "maketrans", b""),
    (bytearray, "maketrans", bytearray()),
    (str, "maketrans", ""),
)

for owner, name, instance in static_methods:
    raw = vars(owner)[name]
    from_class = getattr(owner, name)
    from_instance = getattr(instance, name)
    print(
        owner.__name__,
        name,
        type(raw).__name__,
        raw is vars(owner)[name],
        raw.__func__ is from_class,
        from_class is from_instance,
        name in dir(owner),
        name in dir(instance),
    )

print(int.from_bytes(b"\x01", "big"))
print(float.fromhex("0x1p+1"))
print(bytes.fromhex("4142"))
print(bytearray.fromhex("4142"))
print(dict.fromkeys((1, 2), 9))
print(bytes.maketrans(b"a", b"b")[97])
print(bytearray.maketrans(b"a", b"b")[97])
print(str.maketrans("a", "b")[97])
