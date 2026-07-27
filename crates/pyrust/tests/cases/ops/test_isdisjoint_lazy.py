# isdisjoint must stop at the first common element.  Materialising the whole
# operand would execute the trailing error and break finite-prefix use of
# unbounded iterators.


def source(events, match):
    events.append(1)
    yield match
    events.append("boom")
    raise RuntimeError("too far")


for label, receiver in (
    ("set", {1, 2}),
    ("frozenset", frozenset({1, 2})),
    ("dict-keys", {1: "a", 2: "b"}.keys()),
):
    events = []
    print(label + ":", receiver.isdisjoint(source(events, 2)), events)


# dict_items membership supports unhashable values; isdisjoint must use the
# view's own membership protocol instead of trying to hash the yielded pair.
items = {"a": []}.items()
events = []
print("dict-items:", items.isdisjoint(source(events, ("a", []))), events)


def failing_disjoint(events):
    events.append(8)
    yield 8
    events.append("boom")
    raise RuntimeError("disjoint boom")


events = []
try:
    {1, 2}.isdisjoint(failing_disjoint(events))
except RuntimeError as error:
    print("disjoint error:", events, str(error))


# issuperset has the same short-circuit shape: a missing first item decides
# False without touching a failing tail, while a contained item does not.
for label, receiver in (("set", {1, 2}), ("frozenset", frozenset({1, 2}))):
    events = []
    print(label + " superset:", receiver.issuperset(source(events, 9)), events)

events = []
try:
    {1, 2}.issuperset(source(events, 1))
except RuntimeError as error:
    print("superset error:", events, str(error))
