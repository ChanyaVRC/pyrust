"""
Parity fixture: del/assign module.__dict__ raises AttributeError: readonly attribute
(issue #1665).

CPython 3.12: __dict__ on a module object is a read-only C-level slot exposed
via PyModuleObject.md_dict; attempting to delete or assign it raises
AttributeError('readonly attribute').  pyrust previously raised the wrong error
for deletion, and silently accepted assignment.
"""

import sys
import math

# --- del module.__dict__ raises AttributeError: readonly attribute ---
try:
    del sys.__dict__
    print("del __dict__ raised nothing (WRONG)")
except AttributeError as e:
    print(type(e).__name__, str(e))

# --- same for a different module ---
try:
    del math.__dict__
    print("del math.__dict__ raised nothing (WRONG)")
except AttributeError as e:
    print(type(e).__name__, str(e))

# --- reading module.__dict__ still works after the failed deletion ---
d = sys.__dict__
print(type(d).__name__)

# --- other synthetic dunders are still deletable (PR #1625) ---
del sys.__doc__
print("del __doc__: ok")

# --- second del __dict__ also raises readonly attribute ---
try:
    del sys.__dict__
except AttributeError as e:
    print(type(e).__name__, str(e))

# --- assigning to module.__dict__ also raises readonly attribute (symmetric) ---
try:
    sys.__dict__ = {}
    print("assign __dict__ raised nothing (WRONG)")
except AttributeError as e:
    print(type(e).__name__, str(e))

# --- assigning to math.__dict__ also raises (different module) ---
try:
    math.__dict__ = {}
    print("assign math.__dict__ raised nothing (WRONG)")
except AttributeError as e:
    print(type(e).__name__, str(e))
