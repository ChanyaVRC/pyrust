"""
Parity fixture for issue #1616: module attribute assignment writes to
module.__dict__ instead of raising an error.

CPython 3.12:
  import sys
  sys.foo = 42
  print(sys.foo)  # 42
"""
import sys

# Setting a new attribute on a builtin module
sys.foo = 42
print(sys.foo)

# Overwriting an existing attribute
sys._test_overwrite = 'first'
print(sys._test_overwrite)
sys._test_overwrite = 'second'
print(sys._test_overwrite)

# The written attribute is visible in __dict__
sys._dict_check = 99
print('_dict_check' in sys.__dict__)
print(sys.__dict__['_dict_check'])

# Deleting a module attribute that was set
sys._to_delete = 'bye'
del sys._to_delete
try:
    print(sys._to_delete)
except AttributeError as e:
    print(type(e).__name__)

# Deleting a non-existent module attribute raises AttributeError
try:
    del sys._nonexistent_attr
except AttributeError as e:
    # CPython 3.12 delete-path message: "'module' object has no attribute 'X'"
    print(type(e).__name__, 'no attribute')

# Setting __name__ on a module overwrites the synthetic dunder
import math
math.__name__ = 'math_patched'
print(math.__name__)
math.__name__ = 'math'
print(math.__name__)
