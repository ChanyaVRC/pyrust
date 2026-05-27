# Parity fixture for issue #1255: exception classes must be accessible as
# attributes of the `builtins` module (builtins.ValueError, etc.).

import builtins

# --- Direct attribute access ---
print(builtins.ValueError)
print(builtins.TypeError)
print(builtins.AttributeError)
print(builtins.KeyError)
print(builtins.IndexError)
print(builtins.RuntimeError)
print(builtins.StopIteration)
print(builtins.ImportError)
print(builtins.OSError)
print(builtins.ArithmeticError)
print(builtins.BaseException)
print(builtins.Exception)

# --- hasattr ---
print(hasattr(builtins, 'ValueError'))
print(hasattr(builtins, 'TypeError'))
print(hasattr(builtins, 'AssertionError'))
print(hasattr(builtins, 'NotImplementedError'))
print(hasattr(builtins, 'RecursionError'))
print(hasattr(builtins, 'ZeroDivisionError'))
print(hasattr(builtins, 'OverflowError'))
print(hasattr(builtins, 'GeneratorExit'))
print(hasattr(builtins, 'SystemExit'))
print(hasattr(builtins, 'KeyboardInterrupt'))

# --- dir() membership ---
print('ValueError' in dir(builtins))
print('AssertionError' in dir(builtins))

# --- Identity: builtins.X is the same class object as the bare name ---
print(builtins.ValueError is ValueError)
print(builtins.TypeError is TypeError)
print(builtins.BaseException is BaseException)
print(builtins.OSError is OSError)
# Python 3.3+: IOError and EnvironmentError are aliases for OSError
print(builtins.IOError is OSError)
print(builtins.EnvironmentError is OSError)

# --- getattr() dynamic lookup (framework pattern) ---
print(getattr(builtins, 'ValueError') is ValueError)
print(getattr(builtins, 'NameError') is NameError)

exc_class = getattr(builtins, 'ZeroDivisionError')
print(exc_class.__name__)

# --- Non-existent attribute raises AttributeError ---
try:
    _ = builtins.NoSuchException
    print(False)
except AttributeError:
    print(True)
