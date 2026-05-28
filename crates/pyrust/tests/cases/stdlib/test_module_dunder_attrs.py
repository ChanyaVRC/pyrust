# Parity fixture: module dunder attributes (__name__, __package__, etc.)
# (issue #1350).
#
# CPython 3.12: builtin modules expose __name__ (the module name string) and
# __package__ (empty string for top-level modules).  __loader__ and __spec__
# exist too (both have complex types in CPython; pyrust returns None which is
# acceptable).  __file__ is intentionally absent for builtin modules (CPython
# raises AttributeError for sys.__file__).

import sys
import math

# ── __name__ ─────────────────────────────────────────────────────────────────

print(sys.__name__)    # sys
print(math.__name__)   # math

# ── __package__ ──────────────────────────────────────────────────────────────

print(repr(sys.__package__))   # ''
print(repr(math.__package__))  # ''

# ── __doc__ ──────────────────────────────────────────────────────────────────

# CPython has a real docstring; pyrust returns None.  Both are acceptable.
print(type(sys.__doc__).__name__ in ("str", "NoneType"))    # True
print(type(math.__doc__).__name__ in ("str", "NoneType"))   # True

# ── hasattr checks ────────────────────────────────────────────────────────────

print(hasattr(sys, '__name__'))       # True
print(hasattr(math, '__name__'))      # True
print(hasattr(sys, '__package__'))    # True
print(hasattr(math, '__package__'))   # True
print(hasattr(sys, '__doc__'))        # True
print(hasattr(math, '__doc__'))       # True

# ── dir() includes synthetic dunders ────────────────────────────────────────
# (issue #1350 explicitly mentions `dir(os)` missing dunder attributes)

print('__name__' in dir(sys))       # True
print('__package__' in dir(math))   # True
print('__doc__' in dir(math))       # True
print('__loader__' in dir(math))    # True
print('__spec__' in dir(math))      # True

# ── code pattern from issue body ─────────────────────────────────────────────

m = sys
print(m.__name__ if hasattr(m, '__name__') else '?')   # sys
