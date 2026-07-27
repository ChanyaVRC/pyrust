import itertools


events = []


class Source:
    def __init__(self, name):
        self.name = name

    def __iter__(self):
        events.append(("iter", self.name))
        return iter([self.name])


chained = itertools.chain(Source("first"), Source("second"))
print("chain-created", events)
print("chain-first", next(chained), events)
print("chain-rest", list(chained), events)


class ResumableInner:
    def __init__(self):
        self.raised = False

    def __iter__(self):
        return self

    def __next__(self):
        if not self.raised:
            self.raised = True
            raise RuntimeError("inner boom")
        return 99


inner_error = itertools.chain.from_iterable([ResumableInner(), [7]])
try:
    next(inner_error)
except RuntimeError as exc:
    print("chain-inner-error", str(exc))
print("chain-after-inner-error", next(inner_error))


class ResumableOuter:
    def __init__(self):
        self.raised = False

    def __iter__(self):
        return self

    def __next__(self):
        if not self.raised:
            self.raised = True
            raise RuntimeError("outer boom")
        return [9]


outer_error = itertools.chain.from_iterable(ResumableOuter())
try:
    next(outer_error)
except RuntimeError as exc:
    print("chain-outer-error", str(exc))
try:
    next(outer_error)
except StopIteration:
    print("chain-after-outer-error", "exhausted")


pulled = []


def values():
    for value in range(4):
        pulled.append(value)
        yield value


left, right = itertools.tee(values())
print("tee-created", pulled)
print("tee-left-1", next(left), pulled)
print("tee-left-2", next(left), pulled)
print("tee-right-1", next(right), pulled)
print("tee-right-rest", list(right), pulled)
print("tee-left-rest", list(left), pulled)


zero_events = []


class ZeroSource:
    def __iter__(self):
        zero_events.append("iter")
        return iter([1])


print("tee-zero", itertools.tee(ZeroSource(), 0), zero_events)

infinite_left, infinite_right = itertools.tee(itertools.count())
print("tee-infinite-left", list(itertools.islice(infinite_left, 5)))
print("tee-infinite-right", list(itertools.islice(infinite_right, 3)))
