"""Deque iterators distinguish attempted and effective empty mutations."""

from collections import deque


def outcome(label, iterator):
    try:
        print(label, "value", next(iterator))
    except Exception as exc:
        print(label, type(exc).__name__, repr(str(exc)))


for method in ("append", "appendleft"):
    values = deque(maxlen=0)
    iterator = iter(values)
    getattr(values, method)(1)
    outcome("zero " + method, iterator)

for method in ("extend", "extendleft"):
    values = deque(maxlen=0)
    iterator = iter(values)
    getattr(values, method)([1])
    outcome("zero " + method, iterator)

values = deque()
iterator = iter(values)
values.clear()
outcome("empty clear", iterator)

values = deque([1])
iterator = iter(values)
values.clear()
outcome("nonempty clear", iterator)
