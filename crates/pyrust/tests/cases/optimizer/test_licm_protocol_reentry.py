# LICM must not hoist a Python-protocol operation out of a loop.  The loop is
# empty at runtime, so __add__ must never run.

events = []


class Value:
    def __add__(self, other):
        events.append(other)
        return 0


def run(value, items):
    for item in items:
        value + 40000


run(Value(), [])
print(events)
