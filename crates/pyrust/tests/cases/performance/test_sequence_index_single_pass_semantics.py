items = list(range(10000))
tuple_items = tuple(items)
print(
    "index-primitive",
    items.index(0),
    tuple_items.index(0),
    items.index(9999),
    tuple_items.index(9999, -2),
)

events = []


class Needle:
    def __eq__(self, other):
        events.append(other)
        return other == 2


print("index-target-eq", [0, 1, 2, 3].index(Needle()), events)

events = []


class Element:
    def __eq__(self, other):
        events.append(other)
        return other == 7


print("index-element-eq", [0, Element(), 8].index(7), events)

mutating = []


class ClearOnCompare:
    def __eq__(self, other):
        mutating.clear()
        return False


mutating.extend([ClearOnCompare(), 7])
try:
    mutating.index(7)
except Exception as exc:
    print("index-reentrant", type(exc).__name__, mutating)
