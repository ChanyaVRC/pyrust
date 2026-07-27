"""Dict/set iterators follow live keys across same-size mutation."""


def replace_unvisited(mapping):
    iterator = iter(mapping)
    first = next(iterator)
    del mapping["b"]
    mapping["c"] = 3
    print("dict iterator unvisited", [first] + list(iterator))


replace_unvisited({"a": 1, "b": 2})


mapping = {"a": 1, "b": 2}
loop_values = []
for key in mapping:
    loop_values.append(key)
    if len(loop_values) == 1:
        del mapping["b"]
        mapping["c"] = 3
print("dict loop unvisited", loop_values)


for view_name in ("keys", "values", "items"):
    mapping = {"a": 1, "b": 2}
    iterator = iter(getattr(mapping, view_name)())
    values = [next(iterator)]
    del mapping["b"]
    mapping["c"] = 3
    values.extend(iterator)
    print("dict view", view_name, values)


mapping = {"a": 1, "b": 2}
iterator = iter(mapping)
print("dict seen first", next(iterator))
del mapping["a"]
mapping["c"] = 3
print("dict seen second", next(iterator))
try:
    next(iterator)
except Exception as exc:
    print("dict seen error", type(exc).__name__, str(exc))
try:
    next(iterator)
except Exception as exc:
    print("dict seen after error", type(exc).__name__, str(exc))


class DictSubclass(dict):
    pass


mapping = DictSubclass(a=1, b=2)
iterator = iter(mapping)
subclass_values = [next(iterator)]
del mapping["b"]
mapping["c"] = 3
subclass_values.extend(iterator)
print("dict subclass", subclass_values)


mapping = {value: value for value in range(100)}
iterator = iter(mapping)
prefix = [next(iterator) for _ in range(70)]
del mapping[80]
mapping[100] = 100
suffix = list(iterator)
print(
    "dict adaptive unvisited",
    prefix[-1],
    80 in suffix,
    100 in suffix,
    len(prefix) + len(suffix),
    suffix[:3],
    suffix[-3:],
)


mapping = {value: value for value in range(100)}
iterator = iter(mapping)
prefix = [next(iterator) for _ in range(70)]
del mapping[0]
mapping[100] = 100
suffix = []
try:
    while True:
        suffix.append(next(iterator))
except Exception as exc:
    print(
        "dict adaptive seen",
        suffix[0],
        suffix[-1],
        len(suffix),
        type(exc).__name__,
        str(exc),
    )
try:
    next(iterator)
except Exception as exc:
    print("dict adaptive seen after", type(exc).__name__, str(exc))


mapping = {value: value for value in range(100)}
iterator = iter(mapping.values())
prefix = [next(iterator) for _ in range(70)]
mapping[80] = -80
suffix = list(iterator)
print("dict adaptive value", prefix[-1], suffix[10], len(suffix))


values = {1, 2}
iterator = iter(values)
first = next(iterator)
victim = next(value for value in values if value != first)
values.remove(victim)
values.add(99)
observed = [first] + list(iterator)
print("set unvisited", set(observed) == values, len(observed))


values = {1, 2}
iterator = iter(values)
first = next(iterator)
values.remove(first)
values.add(99)
observed = [first] + list(iterator)
print("set seen", sorted(observed))


values = {1, 2}
loop_values = []
for value in values:
    loop_values.append(value)
    if len(loop_values) == 1:
        values.remove(value)
        values.add(99)
print("set loop seen", sorted(loop_values))


values = set(range(100))
iterator = iter(values)
prefix = [next(iterator) for _ in range(70)]
victim = next(value for value in values if value not in prefix)
values.remove(victim)
values.add(1000)
observed = prefix + list(iterator)
print("set adaptive unvisited", set(observed) == values, len(observed), 1000 in observed)
