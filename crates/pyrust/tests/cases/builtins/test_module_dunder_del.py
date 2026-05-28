"""
Parity fixture: deleting synthetic module dunders should succeed (issue #1625).

CPython 3.12 stores __name__, __package__, __loader__, __spec__, __doc__ in
the real module __dict__; deletion removes them from there.  pyrust synthesised
them on-read without storing them, so delete_attr failed with AttributeError.
"""

import sys
import math

# --- basic deletion of each synthetic dunder succeeds ---
del sys.__name__
print("del __name__: ok")

del sys.__package__
print("del __package__: ok")

del sys.__loader__
print("del __loader__: ok")

del sys.__spec__
print("del __spec__: ok")

del sys.__doc__
print("del __doc__: ok")

# --- hasattr reflects deletion ---
print("hasattr __name__ after del:", hasattr(sys, "__name__"))
print("hasattr __package__ after del:", hasattr(sys, "__package__"))
print("hasattr __loader__ after del:", hasattr(sys, "__loader__"))
print("hasattr __spec__ after del:", hasattr(sys, "__spec__"))

# --- __dict__ does not expose tombstoned dunders ---
del math.__name__
print("__name__ in __dict__ after del:", "__name__" in math.__dict__)

# reassign brings it back in __dict__
math.__name__ = "math"
print("__name__ in __dict__ after reassign:", "__name__" in math.__dict__)

# --- second delete raises AttributeError ---
import os
del os.__name__
try:
    del os.__name__
    print("second del raised nothing (WRONG)")
except AttributeError:
    print("second del raises AttributeError: ok")

# --- delete non-existent regular attr raises AttributeError ---
try:
    del os._no_such_attr_xyz
    print("del nonexistent raised nothing (WRONG)")
except AttributeError:
    print("del nonexistent raises AttributeError: ok")

# --- reassign after delete works ---
import os.path as osp
del osp.__spec__
osp.__spec__ = "custom_spec"
print("__spec__ after del+reassign:", osp.__spec__)
