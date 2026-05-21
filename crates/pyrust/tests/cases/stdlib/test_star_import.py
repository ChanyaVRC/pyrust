# Parity fixture for `from module import *` (issue #926).
# Verifies that star imports populate the caller namespace correctly.

# ── Basic star import from os.path ────────────────────────────────────────────

from os.path import *

# join should be available after star import
print(join("/tmp", "foo"))
print(join("a", "b"))

# basename, dirname, exists, sep are also exported
print(basename("/tmp/foo"))
print(dirname("/tmp/foo"))
print(isinstance(sep, str))

# dir() at module scope returns globals(); imported names must appear there
print("join" in dir())
print("exists" in dir())
print("sep" in dir())

# ── Verify math module star import ────────────────────────────────────────────

from math import *

# pi and e should be available
print(round(pi, 5))
print(round(e, 5))
print(abs(floor(2.7) - 2) == 0)
print(abs(ceil(2.1) - 3) == 0)

# ── Star import followed by regular name lookup ───────────────────────────────

# join came from os.path star import earlier; should still be accessible
# (unless shadowed by math, which has no 'join')
print(join("/a", "b"))

# ── __all__ as list ───────────────────────────────────────────────────────────

from _all_with_list import *

print(pub_a)        # 10
print(pub_b)        # 20
print("_private" not in dir())  # private name must not be imported

# ── __all__ as tuple ──────────────────────────────────────────────────────────

from _all_with_tuple import *

print(pub_c)        # 30
