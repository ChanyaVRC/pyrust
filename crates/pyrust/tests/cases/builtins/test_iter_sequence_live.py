# Explicit list iterators walk the live sequence by index, matching CPython.


values = [1, 2]
iterator = iter(values)
values[0] = 10
values.append(3)
print("before first next:", list(iterator))


values = [0, 1, 2, 3]
iterator = iter(values)
print("remove shift first:", next(iterator))
del values[0]
print("remove shift rest:", list(iterator))


# An iterator keeps its source alive after the original binding is deleted.
values = [4, 5, 6]
iterator = iter(values)
del values
print("keeps source:", next(iterator), list(iterator))


# Once StopIteration has been observed, a later append must not resurrect it.
values = [7]
iterator = iter(values)
print("exhaust first:", next(iterator))
for label in ("initial", "after append"):
    if label == "after append":
        values.append(8)
    try:
        next(iterator)
        print(label, "RESURRECTED")
    except StopIteration:
        print(label, "stopped")


# list subclasses inherit the same live iterator slot.
class ListSubclass(list):
    pass


subclassed = ListSubclass([10, 20])
iterator = iter(subclassed)
subclassed.append(30)
print("subclass:", type(iterator).__name__, list(iterator))

# Every lazy consumer shares the same builtin-subclass classifier as iter().
subclassed = ListSubclass([40, 50])
mapped = map(lambda value: value, subclassed)
subclassed.append(60)
print("subclass through map:", list(mapped))


class SetSubclass(set):
    pass


subclassed_set = SetSubclass([1, 2])
set_iterator = iter(subclassed_set)
subclassed_set.add(3)
try:
    next(set_iterator)
except RuntimeError as error:
    print("set subclass guard:", error)


# The type-level and bound __iter__ entry points share the lazy path.
values = [1, 2]
bound = values.__iter__()
unbound = list.__iter__(values)
values.append(3)
print("dunder bound:", list(bound))
print("dunder unbound:", list(unbound))


# Tuple iterators use the same O(1) source holder, with immutable contents.
tuple_iterator = iter((11, 12, 13))
print("tuple:", type(tuple_iterator).__name__, next(tuple_iterator), list(tuple_iterator))


# enumerate's VM fast path must retain live-list semantics while preserving its
# fused two-target unpack.
values = [10, 20]
seen = []
for index, value in enumerate(values, 5):
    seen.append((index, value))
    if index == 5:
        values.append(30)
print("enumerate live:", seen)


# iter(iterator) returns the same single-pass object.
iterator = iter([1, 2])
print("iterator identity:", iter(iterator) is iterator, next(iterator), list(iterator))
