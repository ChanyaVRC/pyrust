# Verify that importing a missing module raises ModuleNotFoundError (not a bare
# RuntimeError) and that the error message contains the module name.
# Also verifies the exception hierarchy: ModuleNotFoundError is a subclass of
# ImportError (and Exception).

try:
    import nonexistent_pyrust_module_xyz
except ModuleNotFoundError as e:
    print(type(e).__name__)   # ModuleNotFoundError
    print(str(e))             # No module named 'nonexistent_pyrust_module_xyz'

# ModuleNotFoundError must be catchable as ImportError (it's a subclass)
try:
    import nonexistent_pyrust_module_xyz
except ImportError as e:
    print(type(e).__name__)   # ModuleNotFoundError
    print("caught as ImportError")

# Must NOT be caught by a ValueError guard
caught_value_error = False
try:
    import nonexistent_pyrust_module_xyz
except ValueError:
    caught_value_error = True
except ModuleNotFoundError:
    pass
print("caught_value_error:", caught_value_error)
