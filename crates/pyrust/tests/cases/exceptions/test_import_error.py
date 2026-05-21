# Parity fixture: bare `import nonexistent_module` raises ModuleNotFoundError
# (not a bare RuntimeError) and the exception class name is included in the
# printed output. Verifies the CPython 3.12 exception hierarchy:
# ModuleNotFoundError -> ImportError -> Exception.

# Caught as ModuleNotFoundError (exact class name + message).
try:
    import nonexistent_pyrust_module_xyz
except ModuleNotFoundError as e:
    print(type(e).__name__)
    print(str(e))

# ModuleNotFoundError must be catchable as ImportError (it's a subclass).
try:
    import nonexistent_pyrust_module_xyz
except ImportError as e:
    print(type(e).__name__)
    print("caught as ImportError")

# Must NOT be caught by ValueError.
caught_value_error = False
try:
    import nonexistent_pyrust_module_xyz
except ValueError:
    caught_value_error = True
except ModuleNotFoundError:
    pass
print("caught_value_error:", caught_value_error)
