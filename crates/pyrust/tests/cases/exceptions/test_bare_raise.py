# Parity fixture: bare `raise` statement behaviour.
#
# CPython 3.12 raises RuntimeError("No active exception to reraise") when
# `raise` is used with no argument and there is no active exception in scope.

# Case 1: bare raise at module level — caught to inspect message.
try:
    raise
except RuntimeError as e:
    print(repr(str(e)))

# Case 2: bare raise inside a function — no active exception.
def bare_raise_in_function():
    raise

try:
    bare_raise_in_function()
except RuntimeError as e:
    print(repr(str(e)))

# Case 3: bare raise inside an except handler re-raises the active exception.
try:
    try:
        raise ValueError("original")
    except ValueError:
        raise
except ValueError as e:
    print(repr(str(e)))

# Case 4: bare raise correctly re-raises within a nested except.
try:
    raise TypeError("nested")
except TypeError:
    try:
        raise
    except TypeError as e:
        print(repr(str(e)))

print("done")
