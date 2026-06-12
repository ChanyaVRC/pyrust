# Issue #2409: a function whose body can `raise` (or fail an `assert`) must NOT
# be treated as pure.  Pure functions can be memoized / their dead-result calls
# dead-store-eliminated; doing so to a raising function silently swallows the
# exception.  These calls all DISCARD the result, which is exactly the shape the
# eliminator targets — the raise / assert must still fire and be observable.


def reraise(e):
    raise e


# Dead-result call: the exception must still propagate and be catchable.
try:
    reraise(ValueError("boom"))
    print("NOT REACHED")
except ValueError as exc:
    print("caught:", exc)


def check(x):
    assert x, "must be truthy"


# Dead-result call of a function that fails an assert.
try:
    check(0)
    print("NOT REACHED")
except AssertionError as exc:
    print("assert:", exc)


# Conditional raise: pure only on one branch — still must not be eliminated.
def maybe(flag, e):
    if flag:
        raise e
    return 1


try:
    maybe(True, KeyError("k"))
    print("NOT REACHED")
except KeyError as exc:
    print("maybe-raised:", exc)


# Non-raising call to the same helper still works (returns normally).
print("maybe-returned:", maybe(False, KeyError("ignored")))


# Two levels deep, result discarded at the outer call.
def inner(e):
    raise e


def outer(e):
    inner(e)


try:
    outer(IndexError("deep"))
    print("NOT REACHED")
except IndexError as exc:
    print("nested:", exc)
