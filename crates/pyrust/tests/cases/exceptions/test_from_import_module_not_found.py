# from-import of nonexistent module raises ModuleNotFoundError (not RuntimeError).
# CPython 3.12 parity: ModuleNotFoundError is a subclass of ImportError.

# Caught as ModuleNotFoundError (exact class).
try:
    from nonexistent_module import x
except ModuleNotFoundError as e:
    print("caught as ModuleNotFoundError:", e)

# Caught as ImportError (hierarchy — ModuleNotFoundError is a subclass).
try:
    from nonexistent_module import x
except ImportError as e:
    print("caught as ImportError:", e)

# NOT caught as RuntimeError — verify the exception class is correct.
try:
    from nonexistent_module import x
except RuntimeError:
    print("ERROR: caught as RuntimeError")
except ModuleNotFoundError:
    print("not caught as RuntimeError: correct")

# Hierarchy sanity checks.
print(issubclass(ModuleNotFoundError, ImportError))
print(issubclass(ModuleNotFoundError, Exception))
