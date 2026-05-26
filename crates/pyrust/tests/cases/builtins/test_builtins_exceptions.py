"""builtins module exposes exception classes as attributes (issue #1255).

CPython 3.12: every built-in exception class is accessible as an attribute
of the `builtins` module, and the object is the same class used by the
bare name in the global namespace.
"""
import builtins

# Core exception hierarchy
print(builtins.BaseException)
print(builtins.Exception)
print(builtins.ArithmeticError)
print(builtins.LookupError)

# Common exception classes
print(builtins.ValueError)
print(builtins.TypeError)
print(builtins.KeyError)
print(builtins.IndexError)
print(builtins.AttributeError)
print(builtins.NameError)
print(builtins.UnboundLocalError)
print(builtins.RuntimeError)
print(builtins.NotImplementedError)
print(builtins.RecursionError)
print(builtins.OverflowError)
print(builtins.ZeroDivisionError)
print(builtins.StopIteration)
print(builtins.AssertionError)
print(builtins.ImportError)
print(builtins.MemoryError)
print(builtins.SyntaxError)
print(builtins.OSError)
print(builtins.IOError)   # alias for OSError
print(builtins.FileNotFoundError)
print(builtins.PermissionError)
print(builtins.UnicodeError)

# Non-Exception base-exception subclasses
print(builtins.SystemExit)
print(builtins.KeyboardInterrupt)
print(builtins.GeneratorExit)

# Warning hierarchy
print(builtins.Warning)
print(builtins.DeprecationWarning)
print(builtins.RuntimeWarning)
print(builtins.UserWarning)
print(builtins.FutureWarning)
print(builtins.SyntaxWarning)

# Identity: builtins.X is X (same class object)
print(builtins.ValueError is ValueError)
print(builtins.TypeError is TypeError)
print(builtins.Exception is Exception)
print(builtins.int is int)
print(builtins.str is str)

# isinstance and issubclass work through builtins classes
print(isinstance(42, builtins.int))
print(isinstance("hi", builtins.str))
print(issubclass(builtins.ValueError, builtins.Exception))
print(issubclass(builtins.IndexError, builtins.LookupError))
print(issubclass(builtins.FileNotFoundError, builtins.OSError))

# IOError and EnvironmentError are the same class as OSError in Python 3.3+
print(builtins.IOError is builtins.OSError)
print(builtins.EnvironmentError is builtins.OSError)

# Can raise and catch through the builtins module reference
try:
    raise builtins.ValueError("from builtins ref")
except builtins.ValueError as e:
    print("caught:", e)

try:
    raise builtins.KeyError("missing")
except builtins.LookupError:
    print("caught via parent class")
