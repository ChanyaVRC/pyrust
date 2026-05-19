# Bytes and module identity: `is` and id() must use Rc pointer address (issue #722).
#
# CPython object identity for bytes and module objects depends on the backing
# allocation address.  Two aliases of the same bytes object must compare as
# `is True`; two distinct bytes objects must have different ids even if their
# contents are equal.
#
# Note: CPython interns short bytes literals at compile time, so two literals
# with the same content in the same module may share identity.  To test
# distinct-allocation identity we create objects with bytes([...]) at runtime.

# ── bytes identity ────────────────────────────────────────────────────────────

b1 = b"hello"
b2 = b1
print(b1 is b2)       # True  — b2 is an alias of b1
print(b1 is b1)       # True  — self-identity
print(b1 is not b2)   # False

# id() must be non-zero and stable.
print(id(b1) != 0)          # True
print(id(b1) == id(b2))     # True  — same backing object

# Distinct runtime-created bytes objects must have different ids.
b3 = bytes([1, 2, 3])
b4 = bytes([1, 2, 3])
print(b3 is b4)             # False — different allocations
print(b3 is not b4)         # True
print(id(b3) != id(b4))     # True
print(id(b3) != 0)          # True

# Alias of a runtime bytes object.
b5 = bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9])
b6 = b5
print(b5 is b6)             # True
print(id(b5) == id(b6))     # True

# ── module identity ───────────────────────────────────────────────────────────

import sys
import sys as sys2

print(sys is sys2)          # True  — same module object re-imported
print(sys is not sys2)      # False
print(id(sys) == id(sys2))  # True
print(id(sys) != 0)         # True
