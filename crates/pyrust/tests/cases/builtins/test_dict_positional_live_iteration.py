"""Positional live-cursor reads keep CPython's mid-iteration value semantics.

Dict and set walks read entries by position while the backing order is frozen.
The sizes below straddle both the eager key-order capture and the adaptive
capture used by longer walks, so every size class exercises a different cursor
state while producing the same observable output.
"""

SIZES = (0, 1, 2, 8, 63, 64, 65, 128, 600)


for size in SIZES:
    mapping = {value: value * 2 for value in range(size)}
    print(
        "dict walk",
        size,
        sum(mapping),
        sum(mapping.keys()),
        sum(mapping.values()),
        sum(key + value for key, value in mapping.items()),
        len(list(mapping.items())),
    )


for size in SIZES:
    values = set(range(size))
    print("set walk", size, sum(values), len(list(values)))


# A value replaced ahead of the cursor is observed by values() and items().
for size in (2, 8, 64, 65, 200):
    stop = size // 2
    for view_name in ("values", "items"):
        mapping = {value: value for value in range(size)}
        iterator = iter(getattr(mapping, view_name)())
        prefix = [next(iterator) for _ in range(stop)]
        mapping[size - 1] = -1
        mapping[stop] = -2
        suffix = list(iterator)
        print(
            "live overwrite",
            size,
            view_name,
            prefix[-1],
            suffix[0],
            suffix[-1],
            len(prefix) + len(suffix),
        )


# Replacing an already-yielded value does not retroactively change it.
mapping = {"a": 1, "b": 2, "c": 3}
iterator = iter(mapping.items())
first = next(iterator)
mapping["a"] = 100
print("yielded is stable", first, list(iterator))


# The size guard still fires from the positional path, at every size class.
for size in (1, 8, 64, 65, 200):
    mapping = {value: value for value in range(size)}
    try:
        for key in mapping:
            mapping[size + key] = key
    except Exception as exc:
        print("grow guard", size, type(exc).__name__, str(exc))

    values = set(range(size))
    try:
        for value in values:
            values.add(size + value)
    except Exception as exc:
        print("set grow guard", size, type(exc).__name__, str(exc))


# Same-size mutation still restarts the walk with exact seen-key history,
# including for sizes that capture their key order up front.
for size in (8, 64, 65, 128):
    mapping = {value: value for value in range(size)}
    iterator = iter(mapping)
    stop = size // 2
    prefix = [next(iterator) for _ in range(stop)]
    del mapping[size - 1]
    mapping[size] = size
    suffix = list(iterator)
    print(
        "same size recovery",
        size,
        prefix[-1],
        (size - 1) in suffix,
        size in suffix,
        len(prefix) + len(suffix),
        sorted(set(prefix) & set(suffix)),
    )


# Builtin subclasses keep the re-probing cursor: their backing may be replaced.
class Mapping(dict):
    pass


class Values(set):
    pass


for size in (2, 65):
    mapping = Mapping({value: value for value in range(size)})
    iterator = iter(mapping.items())
    next(iterator)
    mapping[size - 1] = -3
    print("subclass items", size, list(iterator)[-1], sum(mapping.values()))

    values = Values(range(size))
    print("subclass set", size, sum(values), len(list(values)))


# Views over a shared mapping advance independently.
mapping = {value: value for value in range(70)}
keys = iter(mapping.keys())
items = iter(mapping.items())
next(keys)
mapping[69] = -4
print("independent views", next(keys), next(items), list(items)[-1])
