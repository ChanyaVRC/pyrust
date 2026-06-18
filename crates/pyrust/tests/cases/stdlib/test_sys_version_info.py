# Parity fixture: sys.version_info supports comparison with tuples (#1223).
#
# sys.version_info must behave like a named tuple: it must be comparable with
# plain tuples using all six rich-comparison operators, indexable, and have
# the expected named attributes.  Exact version numbers (micro, releaselevel)
# differ between CPython and pyrust so only the structural properties are
# tested here.

import sys

# ── basic comparisons ────────────────────────────────────────────────────────
# These hold for any Python 3.x implementation.

print(sys.version_info >= (3,))         # True
print(sys.version_info >= (3, 0))       # True
print(sys.version_info < (4, 0))        # True
print(sys.version_info > (2, 7))        # True
print(sys.version_info <= (4,))         # True
print(sys.version_info != (2, 7))       # True

# ── relational correctness ───────────────────────────────────────────────────
# A 5-element tuple that is definitively less than version_info (major < 3).

print(sys.version_info > (2, 99, 99, 'final', 0))  # True
print(sys.version_info >= (2, 99, 99, 'final', 0)) # True
print((2, 99, 99, 'final', 0) < sys.version_info)  # True (reflected)

# ── attribute access ─────────────────────────────────────────────────────────

print(sys.version_info.major == 3)       # True
print(isinstance(sys.version_info.minor, int))    # True
print(isinstance(sys.version_info.micro, int))    # True
print(sys.version_info.releaselevel == 'final')   # True
print(isinstance(sys.version_info.serial, int))   # True

# ── index access ─────────────────────────────────────────────────────────────

print(sys.version_info[0] == 3)          # True
print(isinstance(sys.version_info[1], int))       # True
print(sys.version_info[-1] == sys.version_info.serial)   # True

# ── length ───────────────────────────────────────────────────────────────────

print(len(sys.version_info) == 5)        # True

# ── type identity ────────────────────────────────────────────────────────────

print(type(sys.version_info).__name__ == 'version_info')    # True
print(type(sys.version_info) == type(sys.version_info))     # True

# ── repr ─────────────────────────────────────────────────────────────────────
# Format: sys.version_info(major=N, minor=N, micro=N, releaselevel='...', serial=N)

r = repr(sys.version_info)
print(r.startswith('sys.version_info('))  # True
print('major=' in r)                      # True
print('minor=' in r)                      # True

# ── slicing and tuple conversion (#2618) ─────────────────────────────────────
# Slicing returns a plain tuple; list()/tuple() yield 5 elements.  Exact
# version numbers differ between implementations, so only structural
# properties are tested.

vi = sys.version_info

print(type(vi[:2]).__name__ == 'tuple')   # True
print(len(vi[:2]) == 2)                   # True
print(len(vi[1:4]) == 3)                  # True
print(len(vi[::-1]) == 5)                 # True
print(len(vi[::2]) == 3)                  # True
print(len(vi[-2:]) == 2)                  # True
print(vi[:2] == (vi[0], vi[1]))           # True
print(len(list(vi)) == 5)                 # True
print(len(tuple(vi)) == 5)                # True

# slice with non-integer member still rejected as TypeError by indexing path
try:
    vi['x']
except TypeError:
    print('TypeError')                    # TypeError
