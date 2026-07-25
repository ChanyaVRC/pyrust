from collections import Counter, defaultdict


# Primitive-key reads and writes use the live backing dict.
counter = Counter({1: 10, "x": 2})
print(
    "counter primitive:",
    counter[1],
    counter[2],
    counter.get("x"),
    counter.get("missing", "fallback"),
    1 in counter,
    2 in counter,
    len(counter),
)
counter[2] = 7
print("counter write:", counter[2], len(counter), list(counter))


factory_calls = []


def factory():
    factory_calls.append("called")
    defaulted["side"] = 9
    return 11


defaulted = defaultdict(factory, {1: 10, "x": 2})
print(
    "defaultdict primitive:",
    defaulted[1],
    defaulted.get("x"),
    defaulted.get("missing", "fallback"),
    1 in defaulted,
    2 in defaulted,
    len(defaulted),
)
print("defaultdict missing:", defaulted[2], defaulted["side"], factory_calls, len(defaulted))


# Object-key equality may re-enter and mutate the same mapping.  The direct
# lookup path must release the backing RefCell borrow before calling __eq__.
class ReentrantKey:
    def __init__(self, value, owner=None):
        self.value = value
        self.owner = owner

    def __hash__(self):
        return 12345

    def __eq__(self, other):
        if self.owner is not None and "eq-side" not in self.owner:
            self.owner["eq-side"] = 99
        return isinstance(other, ReentrantKey) and self.value == other.value


counter_objects = Counter()
stored_counter_key = ReentrantKey("counter", counter_objects)
counter_objects[stored_counter_key] = 3
print(
    "counter object lookup:",
    counter_objects[ReentrantKey("counter")],
    counter_objects["eq-side"],
    len(counter_objects),
)
counter_objects[ReentrantKey("counter")] = 5
print("counter object write:", counter_objects[stored_counter_key], len(counter_objects))


defaultdict_objects = defaultdict(int)
stored_defaultdict_key = ReentrantKey("defaultdict", defaultdict_objects)
defaultdict_objects[stored_defaultdict_key] = 4
print(
    "defaultdict object lookup:",
    defaultdict_objects[ReentrantKey("defaultdict")],
    defaultdict_objects["eq-side"],
    len(defaultdict_objects),
)
defaultdict_objects[ReentrantKey("defaultdict")] = 6
print(
    "defaultdict object write:",
    defaultdict_objects[stored_defaultdict_key],
    len(defaultdict_objects),
)
