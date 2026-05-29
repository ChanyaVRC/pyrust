# collections.abc — abstract base class stubs.
#
# Verifies that `from collections.abc import X` works and that
# `isinstance` checks against the ABCs return the correct values
# for all built-in types.
#
# The parity harness asserts byte-identical output against CPython 3.12,
# so every printed value must be exact.

from collections.abc import (
    Container, Hashable, Iterable, Iterator, Reversible,
    Sized, Callable, Sequence, MutableSequence,
    Set, MutableSet, Mapping, MutableMapping,
)

# ── Sequence ────────────────────────────────────────────────────────────────
print(isinstance([], Sequence))         # True
print(isinstance((), Sequence))         # True
print(isinstance("", Sequence))         # True
print(isinstance(b"", Sequence))        # True
print(isinstance({}, Sequence))         # False
print(isinstance(set(), Sequence))      # False
print(isinstance(42, Sequence))         # False

# ── MutableSequence ─────────────────────────────────────────────────────────
print(isinstance([], MutableSequence))  # True
print(isinstance((), MutableSequence))  # False
print(isinstance("", MutableSequence))  # False

# ── Reversible ──────────────────────────────────────────────────────────────
print(isinstance([], Reversible))       # True
print(isinstance((), Reversible))       # True
print(isinstance("", Reversible))       # True
print(isinstance(b"", Reversible))      # True
print(isinstance({}, Reversible))       # True — dict is reversible in 3.8+
print(isinstance(set(), Reversible))    # False
print(isinstance(frozenset(), Reversible)) # False

# ── Set ─────────────────────────────────────────────────────────────────────
print(isinstance(set(), Set))           # True
print(isinstance(frozenset(), Set))     # True
print(isinstance([], Set))              # False
print(isinstance({}, Set))              # False

# ── MutableSet ──────────────────────────────────────────────────────────────
print(isinstance(set(), MutableSet))       # True
print(isinstance(frozenset(), MutableSet)) # False

# ── Mapping ─────────────────────────────────────────────────────────────────
print(isinstance({}, Mapping))          # True
print(isinstance([], Mapping))          # False
print(isinstance("", Mapping))          # False

# ── MutableMapping ──────────────────────────────────────────────────────────
print(isinstance({}, MutableMapping))   # True
print(isinstance([], MutableMapping))   # False

# ── Container ───────────────────────────────────────────────────────────────
print(isinstance("", Container))        # True
print(isinstance(b"", Container))       # True
print(isinstance([], Container))        # True
print(isinstance((), Container))        # True
print(isinstance({}, Container))        # True
print(isinstance(set(), Container))     # True
print(isinstance(frozenset(), Container)) # True
print(isinstance(42, Container))        # False

# ── Sized ───────────────────────────────────────────────────────────────────
print(isinstance("", Sized))            # True
print(isinstance(b"", Sized))           # True
print(isinstance([], Sized))            # True
print(isinstance((), Sized))            # True
print(isinstance({}, Sized))            # True
print(isinstance(set(), Sized))         # True
print(isinstance(frozenset(), Sized))   # True
print(isinstance(42, Sized))            # False

# ── Iterable ────────────────────────────────────────────────────────────────
print(isinstance("", Iterable))         # True
print(isinstance(b"", Iterable))        # True
print(isinstance([], Iterable))         # True
print(isinstance((), Iterable))         # True
print(isinstance({}, Iterable))         # True
print(isinstance(set(), Iterable))      # True
print(isinstance(frozenset(), Iterable)) # True
print(isinstance(42, Iterable))         # False

# ── Hashable ────────────────────────────────────────────────────────────────
print(isinstance(1, Hashable))          # True
print(isinstance(1.0, Hashable))        # True
print(isinstance("", Hashable))         # True
print(isinstance(b"", Hashable))        # True
print(isinstance((), Hashable))         # True
print(isinstance(frozenset(), Hashable)) # True
print(isinstance(True, Hashable))       # True
print(isinstance(None, Hashable))       # True
print(isinstance([], Hashable))         # False
print(isinstance({}, Hashable))         # False

# ── Callable ────────────────────────────────────────────────────────────────
print(isinstance(len, Callable))        # True
print(isinstance(lambda: None, Callable))  # True
print(isinstance(42, Callable))         # False

# ── Iterator ────────────────────────────────────────────────────────────────
# list itself is not an Iterator (it's Iterable, not Iterator)
print(isinstance([], Iterator))         # False

# ── import via collections.abc ───────────────────────────────────────────────
import collections.abc
print(isinstance([], collections.abc.Sequence))  # True
print(isinstance({}, collections.abc.Mapping))   # True
