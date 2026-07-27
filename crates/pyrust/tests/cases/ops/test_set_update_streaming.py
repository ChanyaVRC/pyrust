# set.update consumes general iterables incrementally.  Mutations completed
# before a source or key error remain visible, and later positional sources are
# not started after an earlier one fails.


events = []


def failing_source():
    for value in (1, 2):
        events.append(value)
        yield value
    raise RuntimeError("source boom")


values = {0}
try:
    values.update(failing_source())
except RuntimeError as error:
    print("source-error:", sorted(values), events, str(error))


def unhashable_source():
    yield 3
    yield []


values = {0}
try:
    values.update(unhashable_source())
except TypeError as error:
    print("key-error:", sorted(values), str(error))


events = []


def untouched_source():
    events.append("started")
    yield 9


values = {0}
try:
    values.update(failing_source(), untouched_source())
except RuntimeError as error:
    print("multi-source:", sorted(values), events, str(error))
