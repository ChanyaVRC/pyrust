from itertools import islice, pairwise


def source(events):
    for value in range(10):
        events.append(value)
        yield value


events = []
sliced = islice(source(events), 5, 6)
print("islice construct:", events)
print("islice first:", next(sliced), events)

# Empty-by-bounds slices still defer their initial start advance until driven.
events = []
empty = islice(source(events), 5, 3)
print("empty construct:", events)
print("empty values:", list(empty), events)

# A large step must not consume beyond stop.
events = []
bounded = islice(source(events), 0, 1, 5)
print("bounded:", list(bounded), events)

# The gap for a non-unit step is consumed by the *following* next(), not
# eagerly before returning the current value.
events = []
stepped = islice(source(events), 0, None, 3)
print("stepped first:", next(stepped), events)
print("stepped second:", next(stepped), events)


class Resumable:
    def __init__(self):
        self.value = 0
        self.raised = False
        self.events = []

    def __iter__(self):
        return self

    def __next__(self):
        if self.value == 1 and not self.raised:
            self.raised = True
            self.events.append("boom")
            raise RuntimeError("boom")
        value = self.value
        self.events.append(value)
        self.value += 1
        return value


# CPython reports a source error once and then leaves the islice exhausted.
resumable = Resumable()
resumed = islice(resumable, 2, None)
try:
    next(resumed)
except RuntimeError as exc:
    print("skip error:", str(exc), resumable.events)
try:
    next(resumed)
except StopIteration:
    print("skip exhausted:", resumable.events)

events = []
pairs = pairwise(source(events))
print("pairwise construct:", events)
print("pairwise first:", next(pairs), events)
