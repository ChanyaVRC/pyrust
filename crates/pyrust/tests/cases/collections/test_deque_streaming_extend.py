from collections import deque


def failing_source():
    yield 4
    yield 5
    raise RuntimeError("boom")


for method in ("extend", "extendleft"):
    values = deque([1, 2, 3], maxlen=4)
    try:
        getattr(values, method)(failing_source())
    except RuntimeError as error:
        print(method, list(values), str(error))


# A zero-sized deque still exhausts the iterable and observes its side effects.
for method in ("extend", "extendleft"):
    events = []

    def source():
        for value in range(3):
            events.append(value)
            yield value

    values = deque(maxlen=0)
    getattr(values, method)(source())
    print(method + "-zero", events, list(values))


# Direct self-extension snapshots the original contents.
for method in ("extend", "extendleft"):
    values = deque([1, 2, 3])
    getattr(values, method)(values)
    print(method + "-self", list(values))


# An already-created iterator is different: the first append invalidates it,
# so the partial mutation remains and RuntimeError propagates.
for method in ("extend", "extendleft"):
    values = deque([1, 2, 3])
    iterator = iter(values)
    try:
        getattr(values, method)(iterator)
    except RuntimeError as error:
        print(method + "-iterator", list(values), str(error))
