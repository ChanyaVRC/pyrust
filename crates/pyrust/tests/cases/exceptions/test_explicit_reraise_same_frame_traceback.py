# Pins the traceback of an explicit `raise e` whose exception already carries
# a (materialised) traceback and is caught again in the frame that re-raised
# it: CPython prepends a node for that frame every time, whatever the length
# of the carried chain (#3384).
import sys
import traceback


def frames(exc):
    out = []
    tb = exc.__traceback__
    while tb is not None:
        out.append(tb.tb_frame.f_code.co_name)
        tb = tb.tb_next
    return out


def make():
    try:
        raise ValueError("carried")
    except ValueError as exc:
        return exc


def deep():
    return make()


print("== module scope")
e = make()
print(frames(e))
try:
    raise e
except ValueError as exc:
    print(frames(exc))
    traceback.print_exc(file=sys.stdout)

e = make()
e.__traceback__ = e.__traceback__.tb_next
print(frames(e))
try:
    raise e
except ValueError as exc:
    print(frames(exc))

print("== function scope")


def reraise_here(materialise):
    e = make()
    if materialise:
        frames(e)
    try:
        raise e
    except ValueError as exc:
        return frames(exc)


def reraise_with_traceback():
    e = make()
    tb = e.__traceback__
    try:
        raise e.with_traceback(tb)
    except ValueError as exc:
        return frames(exc)


def reraise_from():
    e = make()
    frames(e)
    try:
        raise e from None
    except ValueError as exc:
        return frames(exc)


def reraise_twice():
    e = make()
    frames(e)
    out = []
    for _ in range(2):
        try:
            raise e
        except ValueError as exc:
            out.append(len(frames(exc)))
    return out


def reraise_then_propagate():
    e = make()
    frames(e)
    raise e


print(reraise_here(False), reraise_here(True))
print(reraise_with_traceback(), reraise_from(), reraise_twice())
try:
    reraise_then_propagate()
except ValueError as exc:
    print(frames(exc))

print("== with statement")


class Manager:
    def __init__(self):
        self.seen = None

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        self.seen = tb
        return False


def fresh_in_with():
    manager = Manager()
    try:
        with manager:
            raise KeyError("fresh")
    except KeyError as exc:
        return frames(exc), manager.seen is exc.__traceback__


print(fresh_in_with())

print("== a swallowed re-raise does not affect a later exception")


class Swallow:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return True


def swallowed_then_fresh():
    e = make()
    frames(e)
    with Swallow():
        raise e
    manager = Manager()
    try:
        with manager:
            {}["missing"]
    except KeyError as exc:
        return frames(exc), manager.seen is exc.__traceback__


print(swallowed_then_fresh())

print("== bare re-raise keeps the chain")


def bare():
    try:
        try:
            raise ValueError("bare")
        except ValueError:
            raise
    except ValueError as exc:
        return frames(exc)


print(bare())
