import builtins
import sys


# Built-in providers are registered before their Python-source member
# injection so circular imports can observe the partial module.  If that
# injection raises, the partial object must be removed from sys.modules and
# the next import must execute a fresh generation.
sys.modules.pop("collections", None)
saved_property = builtins.property
del builtins.property
try:
    import collections
except NameError:
    print("failed", "collections" in sys.modules)
finally:
    builtins.property = saved_property

import collections

print("retried", "collections" in sys.modules)
print(hasattr(collections, "ChainMap"), hasattr(collections, "UserDict"))
