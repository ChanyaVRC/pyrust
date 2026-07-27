# `yield from` must treat every real StopIteration subclass as iterator
# exhaustion and use its first argument as the delegation expression's value.


class Finished(StopIteration):
    pass


class ReturningIterator:
    def __iter__(self):
        return self

    def __next__(self):
        raise Finished(42)


def delegate():
    result = yield from ReturningIterator()
    print("delegated value:", result)


list(delegate())


# The same classifier drives PEP 479: a real StopIteration subclass escaping a
# generator body is wrapped, with the original exception retained as cause.
def escaping():
    yield "started"
    raise Finished(99)


generator = escaping()
print(next(generator))
try:
    next(generator)
except RuntimeError as exc:
    print(
        "wrapped:",
        str(exc),
        type(exc.__cause__).__name__,
        exc.__cause__.value,
    )


# A user exception merely named StopIteration is not iterator exhaustion.
BuiltinStopIteration = StopIteration


class StopIteration(Exception):
    pass


class LookalikeIterator:
    def __iter__(self):
        return self

    def __next__(self):
        raise StopIteration("not exhausted")


def delegate_lookalike():
    yield from LookalikeIterator()


try:
    next(delegate_lookalike())
except Exception as exc:
    print(
        "lookalike propagated:",
        type(exc).__name__,
        str(exc),
        isinstance(exc, BuiltinStopIteration),
    )


# Materialising an internal StopIteration for PEP 479 must also use the
# canonical built-in class, even after the module global is shadowed.
def builtin_exhaustion():
    yield "started after shadow"
    next(iter(()))


generator = builtin_exhaustion()
print(next(generator))
try:
    next(generator)
except RuntimeError as exc:
    print(
        "shadow-safe cause:",
        isinstance(exc.__cause__, BuiltinStopIteration),
        isinstance(exc.__cause__, StopIteration),
    )
