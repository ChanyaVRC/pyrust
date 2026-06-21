# Issue #2727: modules are inserted into sys.modules *before* their body runs,
# so a circular import reached from inside a module body returns the partial
# module object instead of recursing forever. Previously pyrust stack-overflowed
# on this program; CPython 3.12 runs it cleanly.
#
# Helper modules `_circ_a` / `_circ_b` import each other mutually.
import sys

import _circ_a

# After the dust settles, the fully-initialised module is visible and its
# late-bound attribute is now present.
print("main: _circ_a.a_value =", _circ_a.a_value)
print("main: '_circ_a' in sys.modules:", "_circ_a" in sys.modules)
print("main: '_circ_b' in sys.modules:", "_circ_b" in sys.modules)
print("main: sys.modules['_circ_a'] is _circ_a:", sys.modules["_circ_a"] is _circ_a)
