# Parity fixture: module.__dict__ returns the module namespace dict
# (issue #1338).
#
# CPython 3.12: every module object exposes __dict__, which is the live
# namespace mapping.  pyrust returns a snapshot dict (not a live view)
# which is sufficient for read-only inspection patterns.

import builtins

# builtins.__dict__ returns a dict
d = builtins.__dict__
print(type(d).__name__)  # dict

# Contains builtin names
print('len' in d)        # True
print('print' in d)      # True
print('int' in d)        # True
print('ValueError' in d) # True

# Synthetic dunders are included
print('__name__' in d)   # True

# type(d).__name__ confirms it is a plain dict
print(type(d).__name__ == 'dict')  # True

# Also works on sys
import sys
sd = sys.__dict__
print(type(sd).__name__)  # dict
print('path' in sd)       # True (sys.path exists)
print('version' in sd)    # True
