# collections.abc — structural subtyping via __subclasshook__.
#
# Verifies that user-defined classes implementing the required abstract methods
# are recognised as ABC members without explicit registration, matching
# CPython 3.12's __subclasshook__ / _check_methods behaviour.
#
# Also verifies that bytearray is correctly recognised as Iterable, Sized,
# Container, Reversible, Sequence, and MutableSequence.

from collections.abc import (
    Callable, Container, Hashable, Iterable, Iterator,
    Generator, Reversible, Sized, Sequence, MutableSequence,
    Set, MutableSet, Mapping, MutableMapping,
)

# ── User-defined classes ─────────────────────────────────────────────────────

class WithIter:
    def __iter__(self):
        return iter([])

class WithLen:
    def __len__(self):
        return 0

class WithContains:
    def __contains__(self, x):
        return False

class WithCall:
    def __call__(self):
        pass

class WithReversed:
    def __reversed__(self):
        return iter([])

class WithIterAndNext:
    def __iter__(self):
        return self
    def __next__(self):
        raise StopIteration

class WithHash:
    def __hash__(self):
        return 42

class WithHashNone:
    __hash__ = None

class Empty:
    pass

# ── Structural checks ────────────────────────────────────────────────────────

print(isinstance(WithIter(), Iterable))         # True
print(isinstance(WithLen(), Sized))             # True
print(isinstance(WithContains(), Container))    # True
print(isinstance(WithCall(), Callable))         # True

# Iterator requires both __iter__ and __next__
print(isinstance(WithIterAndNext(), Iterator))  # True
print(isinstance(WithIter(), Iterator))         # False — only __iter__

# Reversible requires __reversed__ AND __iter__ (CPython _check_methods)
# A class with only __reversed__ but no __iter__ is not Reversible.
class OnlyReversed:
    def __reversed__(self):
        return iter([])

print(isinstance(WithReversed(), Reversible))   # False — no __iter__

class WithIterAndReversed:
    def __iter__(self): return iter([])
    def __reversed__(self): return iter([])

print(isinstance(WithIterAndReversed(), Reversible))  # True

# ── Hashable edge cases ──────────────────────────────────────────────────────

# User class with explicit __hash__ → Hashable
print(isinstance(WithHash(), Hashable))         # True

# User class with __hash__ = None → NOT Hashable (explicitly excluded)
print(isinstance(WithHashNone(), Hashable))     # False

# User class with no __hash__ → Hashable (inherits object.__hash__)
print(isinstance(Empty(), Hashable))            # True

# Primitive types with no __hash__ (list, dict, set)
print(isinstance([], Hashable))                 # False
print(isinstance({}, Hashable))                 # False
print(isinstance(set(), Hashable))              # False

# ── bytearray ────────────────────────────────────────────────────────────────

print(isinstance(bytearray(), Iterable))        # True
print(isinstance(bytearray(), Sized))           # True
print(isinstance(bytearray(), Container))       # True
print(isinstance(bytearray(), Reversible))      # True
print(isinstance(bytearray(), Sequence))        # True
print(isinstance(bytearray(), MutableSequence)) # True
print(isinstance(bytearray(), Hashable))        # False

# ── range ────────────────────────────────────────────────────────────────────

print(isinstance(range(5), Iterable))           # True
print(isinstance(range(5), Sized))              # True
print(isinstance(range(5), Hashable))           # True
print(isinstance(range(5), Reversible))         # True

# ── Generator structural check ───────────────────────────────────────────────

def gen():
    yield 1

g = gen()
print(isinstance(g, Generator))                 # True
print(isinstance(g, Iterator))                  # True
print(isinstance(g, Iterable))                  # True

# ── Regression: non-structural ABCs still rely on registration ───────────────
# Sequence, Mapping, etc. have no direct structural hook in CPython.
# These should still work for registered primitive types.

print(isinstance([], Sequence))                 # True
print(isinstance("", Sequence))                 # True
print(isinstance({}, Mapping))                  # True
print(isinstance({}, MutableMapping))           # True
print(isinstance(set(), Set))                   # True

# User-defined class implementing all Mapping methods does NOT automatically
# become a Mapping — Mapping.__subclasshook__ returns NotImplemented in CPython
# (the hook checks cls is Collection which Mapping inherits from, so it fires
# but 'cls is Collection' is False, returning NotImplemented).
class MyMapping:
    def __getitem__(self, k): raise KeyError
    def __iter__(self): return iter([])
    def __len__(self): return 0

print(isinstance(MyMapping(), Mapping))         # False

# ── int is not Iterable, but is Hashable ─────────────────────────────────────

print(isinstance(42, Iterable))                 # False
print(isinstance(42, Hashable))                 # True
print(isinstance(42, Sized))                    # False
print(isinstance(42, Callable))                 # False
