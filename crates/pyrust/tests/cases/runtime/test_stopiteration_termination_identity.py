# Every iterator consumer must recognise the canonical StopIteration class by
# identity/subclass relationship, not by the Python-visible class name.

import itertools
import functools


BuiltinStopIteration = StopIteration


class Finished(BuiltinStopIteration):
    def __init__(self, value):
        super().__init__(value + "!")


class StopIteration(Exception):
    def __init__(self, value):
        super().__init__(value + "!")


class RaisingIterator:
    def __init__(self, error_type):
        self.error_type = error_type

    def __iter__(self):
        return self

    def __next__(self):
        raise self.error_type("done")


class RaisingSequence:
    def __init__(self, error_type):
        self.error_type = error_type

    def __getitem__(self, index):
        raise self.error_type("done")


def report(label, operation):
    try:
        print(label, operation())
    except Exception as exc:
        print(
            label,
            "raised",
            type(exc).__name__,
            isinstance(exc, BuiltinStopIteration),
            str(exc),
        )


def exception_value_shape(exc):
    try:
        return ("value", exc.value, exc.args)
    except AttributeError:
        return ("no-value", exc.args)


print("metadata-subclass", exception_value_shape(Finished("meta")))
print("metadata-lookalike", exception_value_shape(StopIteration("meta")))


def next_default(error_type):
    return next(RaisingIterator(error_type), "default")


def materialise(error_type):
    return list(RaisingIterator(error_type))


def aggregate(error_type):
    return sum(RaisingIterator(error_type))


def any_source(error_type):
    return any(RaisingIterator(error_type))


def all_source(error_type):
    return all(RaisingIterator(error_type))


def membership(error_type):
    return 1 in RaisingIterator(error_type)


def for_loop(error_type):
    for _ in RaisingIterator(error_type):
        return "item"
    return "done"


def extend_list(error_type):
    result = [1]
    result.extend(RaisingIterator(error_type))
    return result


def update_dict(error_type):
    result = {"kept": 1}
    result.update(RaisingIterator(error_type))
    return result


def map_source(error_type):
    return list(map(lambda value: value, RaisingIterator(error_type)))


def zip_source(error_type):
    return list(zip(RaisingIterator(error_type), [1]))


def chain_source(error_type):
    return list(itertools.chain(RaisingIterator(error_type)))


def islice_source(error_type):
    return list(itertools.islice(RaisingIterator(error_type), 2))


def reduce_source(error_type):
    return functools.reduce(lambda left, right: left + right, RaisingIterator(error_type), 10)


def reduce_empty_source(error_type):
    return functools.reduce(lambda left, right: left + right, RaisingIterator(error_type))


def legacy_sequence(error_type):
    return list(RaisingSequence(error_type))


def returning_generator():
    yield "inner-ready"
    return 42


def delegate_return():
    result = yield from returning_generator()
    yield ("returned", result)


def catching_generator():
    try:
        yield "throw-ready"
    except ValueError:
        return 73


def delegate_throw():
    result = yield from catching_generator()
    yield ("throw-returned", result)


def drive_delegated_throw():
    generator = delegate_throw()
    first = next(generator)
    second = generator.throw(ValueError("boom"))
    return [first, second]


operations = [
    ("next", next_default),
    ("list", materialise),
    ("sum", aggregate),
    ("any", any_source),
    ("all", all_source),
    ("membership", membership),
    ("for", for_loop),
    ("extend", extend_list),
    ("dict-update", update_dict),
    ("map", map_source),
    ("zip", zip_source),
    ("chain", chain_source),
    ("islice", islice_source),
    ("reduce-initial", reduce_source),
    ("reduce-empty", reduce_empty_source),
    ("getitem", legacy_sequence),
]

for label, operation in operations:
    report(label + "-subclass", lambda op=operation: op(Finished))
    report(label + "-lookalike", lambda op=operation: op(StopIteration))

report("yield-from-shadow-return", lambda: list(delegate_return()))
report("yield-from-shadow-throw", drive_delegated_throw)
