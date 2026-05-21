# Helper module for test_star_import_errors.py.
# Declares __all__ with a non-string element (should raise TypeError).
__all__ = ["exported", 99]
exported = 1
