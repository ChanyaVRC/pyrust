"""
Parity fixture: AttributeError message format after deleting module __name__ (issue #1687).

CPython 3.12 module_getattro builds the error message by reading __name__ from the
module's __dict__.  When __name__ has been deleted, it cannot retrieve the module name
and omits it from the error: "module has no attribute 'X'" instead of the usual
"module 'foo' has no attribute 'X'".  This applies to every attr lookup that fails
after __name__ is deleted.
"""

import sys
import math
import os

# --- Tombstone path: del __name__ then access __name__ ---
del sys.__name__
try:
    _ = sys.__name__
except AttributeError as e:
    print(str(e))

# --- Tombstone path: after del __name__, access nonexistent attr ---
# __name__ is gone => module name omitted from error
try:
    _ = sys.nonexistent_attr_xyz
except AttributeError as e:
    print(str(e))

# --- Tombstone path: del other synthetic dunder (NOT __name__) ---
# __name__ still present => module name appears in error
del math.__package__
try:
    _ = math.__package__
except AttributeError as e:
    print(str(e))

# --- Normal missing attr with __name__ intact ---
try:
    _ = math.nonexistent_attr_xyz
except AttributeError as e:
    print(str(e))

# --- del __name__ then also del __package__: both should show unquoted form ---
del os.__name__
del os.__package__
try:
    _ = os.__package__
except AttributeError as e:
    print(str(e))
