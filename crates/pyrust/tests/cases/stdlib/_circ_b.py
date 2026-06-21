# Helper module for test_circular_import.py (issue #2727).
import sys

# b is reached from inside a's body. CPython has already inserted the partial
# `_circ_a` module into sys.modules before running its body, so this is True and
# the attribute set after this import (`a_value`) is not yet visible.
print("b: '_circ_a' in sys.modules:", "_circ_a" in sys.modules)
import _circ_a

print("b: _circ_a.a_value =", getattr(_circ_a, "a_value", "<MISSING>"))
