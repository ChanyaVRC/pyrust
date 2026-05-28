# Relative imports must parse without SyntaxError.
# In a non-package context (standalone script) CPython 3.12 raises ImportError
# at runtime.  pyrust has no package system so we accept the same runtime error.

try:
    exec("from . import x")
except (ImportError, ModuleNotFoundError, SystemError):
    print("runtime_error_ok")
except SyntaxError:
    print("syntax_error_bad")

try:
    exec("from .utils import helper")
except (ImportError, ModuleNotFoundError, SystemError):
    print("runtime_error_ok")
except SyntaxError:
    print("syntax_error_bad")

try:
    exec("from .. import parent")
except (ImportError, ModuleNotFoundError, SystemError):
    print("runtime_error_ok")
except SyntaxError:
    print("syntax_error_bad")

# Absolute imports continue to work
try:
    exec("from sys import version_info")
    print("absolute_ok")
except Exception:
    print("absolute_bad")
