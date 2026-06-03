"""
Parity fixture: islice() raises ValueError (not TypeError) for non-integer
arguments and for out-of-range integers, with CPython 3.12 exact messages.
"""
import itertools


def check(desc, fn):
    try:
        fn()
        print(f"{desc}: no error")
    except Exception as e:
        print(f"{desc}: {type(e).__name__}: {e}")


# Non-integer stop argument (2-arg form)
check("stop='a'", lambda: list(itertools.islice(range(10), 'a')))

# Non-integer start argument (3-arg form)
check("start='a'", lambda: list(itertools.islice(range(10), 'a', 5)))

# Non-integer stop argument (3-arg form)
check("stop='a' 3-arg", lambda: list(itertools.islice(range(10), 0, 'a')))

# Non-integer step argument
check("step='a'", lambda: list(itertools.islice(range(10), 0, 10, 'a')))

# Negative stop (out of range); use -2 to avoid CPython's -1 sentinel quirk
check("stop=-2", lambda: list(itertools.islice(range(10), -2)))

# Negative start (out of range)
check("start=-1", lambda: list(itertools.islice(range(10), -1, 5)))

# Step of zero
check("step=0", lambda: list(itertools.islice(range(10), 0, 10, 0)))

# Negative step
check("step=-1", lambda: list(itertools.islice(range(10), 0, 10, -1)))

# An __index__ object that returns a non-int, or raises, is reported with the
# slot ValueError (CPython clears the inner error) — not the generic
# "__index__ returned non-int" TypeError that the other index contexts use.
class NonIntIndex:
    def __index__(self):
        return "x"


class RaisingIndex:
    def __index__(self):
        raise ValueError("boom")


class NegIndex:
    def __index__(self):
        return -2


check("stop=NonIntIndex", lambda: list(itertools.islice(range(10), NonIntIndex())))
check("start=NonIntIndex", lambda: list(itertools.islice(range(10), NonIntIndex(), 5)))
check("step=NonIntIndex", lambda: list(itertools.islice(range(10), 0, 10, NonIntIndex())))
check("stop=RaisingIndex", lambda: list(itertools.islice(range(10), RaisingIndex())))
check("stop=NegIndex", lambda: list(itertools.islice(range(10), NegIndex())))
check("start=NegIndex", lambda: list(itertools.islice(range(10), NegIndex(), 5)))

# Happy paths — ensure they still work
print(list(itertools.islice(range(10), 3)))
print(list(itertools.islice(range(10), 2, 5)))
print(list(itertools.islice(range(10), 0, 10, 3)))
print(list(itertools.islice(range(10), None)))
print(list(itertools.islice(range(10), 2, None)))
print(list(itertools.islice(range(10), 0, None, 2)))
