# Parity fixture: __import__ is registered as a builtin.
# CPython 3.12 acceptance criteria (issue #1247):
#   - __import__("os") returns the os module
#   - __import__("os.path") returns the top-level os module (empty fromlist)
#   - import builtins; builtins.__import__ resolves to the function
#   - __import__("os.path", fromlist=["sep"]) returns os.path (non-empty fromlist)

# 1. Basic single-name import.
m = __import__("os")
print(type(m).__name__)          # module

# 2. Dotted name with empty fromlist returns top-level package.
m2 = __import__("os.path")
print(type(m2).__name__)         # module
# Both should be the same top-level module (os).
print(m2 is m)                   # True

# 3. __import__ is present in the builtins module.
import builtins
print(hasattr(builtins, "__import__"))   # True
print(callable(builtins.__import__))     # True

# 4. Non-empty fromlist: return the leaf (os.path), not the top.
m3 = __import__("os.path", fromlist=["sep"])
print(type(m3).__name__)         # module
print(m3 is m)                   # False -- different module (os.path != os)

# 5. The returned module from basic import is accessible and functional.
sep = __import__("os").path.sep
print(type(sep).__name__)        # str
print(len(sep) > 0)              # True -- sep is "/" on Linux
