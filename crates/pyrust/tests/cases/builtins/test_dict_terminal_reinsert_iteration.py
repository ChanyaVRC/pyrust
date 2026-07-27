"""A dict iterator distinguishes entry reinsertion from unrelated mutation."""


def consume_and_reinsert(label, iterator_factory, size):
    mapping = {value: value for value in range(size)}
    iterator = iterator_factory(mapping)
    list(iterator)
    final_key = size - 1
    del mapping[final_key]
    mapping[final_key] = -1
    try:
        next(iterator)
    except Exception as exc:
        print(label, size, type(exc).__name__, str(exc))
    try:
        next(iterator)
    except Exception as exc:
        print(label, size, "after", type(exc).__name__, str(exc))


for size in (1, 5, 10, 21, 42, 64, 65, 100):
    consume_and_reinsert("direct", iter, size)

for label, factory in (
    ("keys", lambda mapping: iter(mapping.keys())),
    ("values", lambda mapping: iter(mapping.values())),
    ("items", lambda mapping: iter(mapping.items())),
):
    consume_and_reinsert(label, factory, 100)


class DictSubclass(dict):
    pass


mapping = DictSubclass({value: value for value in range(100)})
iterator = iter(mapping)
list(iterator)
del mapping[99]
mapping[99] = -1
try:
    next(iterator)
except Exception as exc:
    print("subclass", type(exc).__name__, str(exc))


mapping = {0: 0}
iterator = iter(mapping)
next(iterator)
mapping[1] = 1
del mapping[1]
try:
    next(iterator)
except Exception as exc:
    print("temporary", type(exc).__name__, str(exc))


mapping = {0: 0}
iterator = iter(mapping)
next(iterator)
old = list(mapping.items())
mapping.clear()
mapping.update(old)
try:
    next(iterator)
except Exception as exc:
    print("clear restore", type(exc).__name__, str(exc))
