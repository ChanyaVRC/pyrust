from collections import Counter, defaultdict, deque


def failing_values():
    yield "a"
    yield "b"
    yield "a"
    raise RuntimeError("values boom")


counter = Counter(old=9)
try:
    counter.__init__(failing_values())
except RuntimeError as exc:
    print("counter reinit:", str(exc), dict(counter))


def failing_pairs():
    yield ("a", 1)
    yield ("b", 2)
    raise RuntimeError("pairs boom")


defaulted = defaultdict(list, old=9)
try:
    defaulted.__init__(tuple, failing_pairs())
except RuntimeError as exc:
    print(
        "defaultdict reinit:",
        str(exc),
        defaulted.default_factory is tuple,
        dict(defaulted),
    )


class FailingMapping:
    def keys(self):
        return ["a", "b", "c"]

    def __getitem__(self, key):
        if key == "b":
            raise RuntimeError("getitem boom")
        return key.upper()


mapped = defaultdict(list, old=9)
try:
    mapped.__init__(tuple, FailingMapping())
except RuntimeError as exc:
    print(
        "defaultdict mapping:",
        str(exc),
        mapped.default_factory is tuple,
        dict(mapped),
    )


def failing_deque(events):
    for value in (4, 5):
        events.append(value)
        yield value
    events.append("boom")
    raise RuntimeError("deque boom")


events = []
bounded = deque([1, 2, 3], maxlen=3)
try:
    bounded.__init__(failing_deque(events), maxlen=2)
except RuntimeError as exc:
    print("deque reinit:", str(exc), list(bounded), bounded.maxlen, events)

events = []
zero = deque([1, 2, 3], maxlen=3)
try:
    zero.__init__(failing_deque(events), maxlen=0)
except RuntimeError as exc:
    print("deque zero:", str(exc), list(zero), zero.maxlen, events)

self_source = deque([1, 2, 3])
self_source.__init__(self_source)
print("deque self:", list(self_source))


class BadIterable:
    def __iter__(self):
        raise RuntimeError("iter boom")


bad_iter = deque([1, 2, 3], maxlen=3)
try:
    bad_iter.__init__(BadIterable(), maxlen=2)
except RuntimeError as exc:
    print("deque iter error:", str(exc), list(bad_iter), bad_iter.maxlen)

guarded = deque([1, 2, 3])
old_iterator = iter(guarded)
guarded.__init__([7, 8])
try:
    next(old_iterator)
except RuntimeError as exc:
    print("deque old iterator:", str(exc), list(guarded))
