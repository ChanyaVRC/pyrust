# Parity fixture for __import__ builtin (issue #1247).
# CPython 3.12 acceptance criteria:
#   - __import__('os') returns the os module
#   - __import__('os.path') returns top-level os (empty fromlist)
#   - __import__('os.path', fromlist=['join']) returns os.path (non-empty fromlist)
#   - builtins.__import__ is callable
#   - ValueError on empty name
#   - TypeError on non-string name
#   - ModuleNotFoundError on nonexistent module

# 1. Basic single-name import.
m = __import__('os')
print(type(m).__name__)          # module
print(m.__name__)                # os

# 2. Dotted name with empty fromlist returns top-level package.
m2 = __import__('os.path')
print(type(m2).__name__)         # module
print(m2.__name__)               # os

# 3. Non-empty fromlist: return the leaf module.
m3 = __import__('os.path', fromlist=['join'])
print(type(m3).__name__)         # module
# The leaf module is different from the top-level os.
print(m3 is m)                   # False

# 4. __import__ is present in the builtins module.
import builtins
print(callable(builtins.__import__))   # True

# 5. ValueError on empty name.
try:
    __import__('')
except ValueError as e:
    print('ValueError:', e)

# 6. TypeError on non-string name.
try:
    __import__(42)
except TypeError as e:
    print('TypeError:', e)

# 7. ModuleNotFoundError on nonexistent module.
try:
    __import__('_pyrust_nonexistent_module')
except ModuleNotFoundError as e:
    print('ModuleNotFoundError:', e)
