# collections.abc — missing ABC registrations for user functions and range.
#
# Issue #1793: user-defined functions and bound methods should be recognised
# as Hashable (they are hashable by identity in CPython).
#
# Issue #1800: range is registered as a Sequence (and Reversible) in CPython's
# Lib/_collections_abc.py; pyrust was missing that registration.

from collections.abc import (
    Callable,
    Container,
    Hashable,
    Iterable,
    Reversible,
    Sequence,
    Sized,
)

# ── Issue #1793: user functions as Hashable ───────────────────────────────────

def f():
    pass

g = lambda: None


# isinstance checks
print(isinstance(f, Hashable))      # True
print(isinstance(g, Hashable))      # True
print(isinstance(f, Callable))      # True
print(isinstance(g, Callable))      # True

# issubclass checks via function/method class
print(issubclass(type(f), Hashable))    # True
print(issubclass(type(g), Hashable))    # True
print(issubclass(type(f), Callable))    # True

# hash() must not raise
print(isinstance(hash(f), int))     # True
print(isinstance(hash(g), int))     # True

# Bound user methods are also hashable
class C:
    def m(self):
        pass

c = C()
print(isinstance(c.m, Hashable))        # True
print(isinstance(c.m, Callable))        # True
print(issubclass(type(c.m), Hashable))  # True
print(isinstance(hash(c.m), int))       # True

# ── Issue #1800: range as Sequence ───────────────────────────────────────────

r = range(5)

print(isinstance(r, Sequence))      # True
print(isinstance(r, Reversible))    # True
print(isinstance(r, Iterable))      # True
print(isinstance(r, Sized))         # True
print(isinstance(r, Container))     # True
print(isinstance(r, Hashable))      # True  — range is hashable

# issubclass checks (range as a class)
print(issubclass(range, Sequence))      # True
print(issubclass(range, Reversible))    # True
print(issubclass(range, Iterable))      # True
print(issubclass(range, Sized))         # True
print(issubclass(range, Container))     # True
print(issubclass(range, Hashable))      # True

# Calling range() still works correctly after it became a PyClass
print(list(range(3)))               # [0, 1, 2]
print(list(range(1, 5)))            # [1, 2, 3, 4]
print(list(range(0, 10, 3)))        # [0, 3, 6, 9]

# type() of a range value
print(type(range(5)).__name__)      # range

# ── No regression: other non-hashable types still False ───────────────────────

print(isinstance([], Hashable))     # False
print(isinstance({}, Hashable))     # False
print(isinstance(set(), Hashable))  # False
