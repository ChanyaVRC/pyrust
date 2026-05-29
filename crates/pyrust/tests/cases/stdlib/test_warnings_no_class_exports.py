# Regression test for issue #1752: the `warnings` module must NOT export
# warning category classes (UserWarning, DeprecationWarning, etc.) as module
# attributes.  CPython 3.12: hasattr(warnings, 'UserWarning') is False.
# Those names live in builtins, not in the module namespace.

import warnings

# Warning classes must not be module attributes.
print(hasattr(warnings, 'UserWarning'))        # False
print(hasattr(warnings, 'DeprecationWarning')) # False
print(hasattr(warnings, 'Warning'))            # False
print(hasattr(warnings, 'RuntimeWarning'))     # False

# Module-level classes that DO belong to warnings.
print(hasattr(warnings, 'catch_warnings'))     # True
print(hasattr(warnings, 'WarningMessage'))     # True
print(hasattr(warnings, 'filters'))            # True

# warn() still works — the class is resolved from the caller's builtins namespace.
with warnings.catch_warnings():
    warnings.simplefilter("ignore")
    warnings.warn("test", UserWarning)
    warnings.warn("test2")
print("warn-ok")  # warn-ok

# The classes remain accessible as builtins (not via the module).
print(UserWarning.__name__)        # UserWarning
print(DeprecationWarning.__name__) # DeprecationWarning
