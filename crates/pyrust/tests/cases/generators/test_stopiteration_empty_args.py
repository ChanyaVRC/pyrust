# Parity fixture: a generator that returns None (falls off the end, bare
# `return`, or `return None`) must synthesise `StopIteration()` with *empty*
# args, matching CPython 3.12.  Only a non-None return value is stored as the
# single arg.  `.value` is `None` in the empty-args case either way.


def fell_off():
    yield 1


def bare_return():
    yield 1
    return


def return_none():
    yield 1
    return None


def return_value():
    yield 1
    return 7


def drain(make):
    g = make()
    next(g)  # consume the single yield
    try:
        next(g)
    except StopIteration as e:
        return (e.args, e.value)


print(drain(fell_off))       # ((), None)
print(drain(bare_return))    # ((), None)
print(drain(return_none))    # ((), None)
print(drain(return_value))   # ((7,), 7)

# Empty (never-yielding) generator: first next() raises StopIteration() too.
empty = (x for x in [])
try:
    next(empty)
except StopIteration as e:
    print(e.args, e.value)   # () None

# PEP 380: `yield from` still extracts the sub-generator's return value even
# though the synthesised StopIteration now carries empty args for None.
def sub_none():
    return
    yield


def sub_value():
    return 99
    yield


def driver(make):
    received = yield from make()
    print("received", received)


list(driver(sub_none))   # received None
list(driver(sub_value))  # received 99
