# collections.abc.Hashable — built-in functions and bound methods.
#
# Verifies that built-in functions (`len`, `print`) and built-in bound
# methods (`[].append`, `"".upper`, `{}.get`) are recognised as Hashable
# and that hash() on them does not raise TypeError.
#
# Issue #1770: pyrust returned False for isinstance(len, Hashable) because
# the BuiltinFunction arm in isinstance_single only handled Callable.

from collections.abc import Hashable, Callable

# ── isinstance checks ────────────────────────────────────────────────────────

# Built-in functions
print(isinstance(len, Hashable))        # True
print(isinstance(print, Hashable))      # True
print(isinstance(abs, Hashable))        # True

# Built-in bound methods
print(isinstance([].append, Hashable))  # True
print(isinstance("".upper, Hashable))   # True
print(isinstance({}.get, Hashable))     # True

# Callable still True for both (no regression)
print(isinstance(len, Callable))        # True
print(isinstance([].append, Callable))  # True

# Non-hashable types should remain False
print(isinstance([], Hashable))         # False
print(isinstance({}, Hashable))         # False
print(isinstance(set(), Hashable))      # False

# ── hash() does not raise ────────────────────────────────────────────────────

print(isinstance(hash(len), int))       # True
print(isinstance(hash(print), int))     # True

a = []
h1 = hash(a.append)
h2 = hash(a.append)
print(isinstance(h1, int))             # True
print(h1 == h2)                        # True — stable across calls on same object

# Different receivers produce different hashes
b = []
print(hash(a.append) == hash(b.append))  # False — different list objects
