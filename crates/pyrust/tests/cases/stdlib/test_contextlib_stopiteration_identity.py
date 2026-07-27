# contextlib's generator protocol must recognise the canonical StopIteration
# hierarchy. A renamed proper subclass still means exhaustion; an unrelated
# exception merely named StopIteration must propagate.

import contextlib


class Finished(StopIteration):
    pass


Finished.__name__ = "RenamedFinished"


class FinishingIterator:
    def __init__(self):
        self.started = False

    def __iter__(self):
        return self

    def __next__(self):
        if not self.started:
            self.started = True
            return "entered"
        raise Finished("complete")


def finishing_source():
    return FinishingIterator()


manager = contextlib.contextmanager(finishing_source)()
print("subclass enter:", manager.__enter__())
print("subclass exit:", manager.__exit__(None, None, None))


BuiltinStopIteration = StopIteration


class StopIteration(Exception):
    pass


class LookalikeIterator:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration("not exhausted")


def lookalike_source():
    return LookalikeIterator()


manager = contextlib.contextmanager(lookalike_source)()
try:
    manager.__enter__()
except Exception as exc:
    print(
        "lookalike propagated:",
        type(exc).__name__,
        str(exc),
        isinstance(exc, BuiltinStopIteration),
    )
else:
    print("lookalike propagated: no")
