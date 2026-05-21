# Parity fixture for copy.Error — issue #896.
#
# CPython's `copy` module exports `copy.Error` as a subclass of `Exception`.
#
# NOTE: `print(copy.Error)` is intentionally omitted: CPython shows
# `<class 'copy.Error'>` (module-qualified) while pyrust's class repr uses
# `name` not `qualname`, producing `<class 'Error'>`.  That pre-existing repr
# divergence is tracked separately; this fixture focuses on the attributes that
# matter for the acceptance criteria of issue #896.

import copy

# The class is accessible on the module (no AttributeError).
print(type(copy.Error).__name__)   # -> "type"

# Name attribute matches CPython.
print(copy.Error.__name__)         # -> "Error"

# Subclass relationships hold.
print(issubclass(copy.Error, Exception))      # -> True
print(issubclass(copy.Error, BaseException))  # -> True

# Instances are recognised as exceptions.
e = copy.Error("something went wrong")
print(isinstance(e, Exception))    # -> True
print(isinstance(e, copy.Error))   # -> True

# Instance with no args.
e0 = copy.Error()
print(isinstance(e0, copy.Error))  # -> True

# raise / except round-trip using the specific class.
try:
    raise copy.Error("oops")
except copy.Error as exc:
    print("caught:", exc)          # -> caught: oops

# copy.Error is also catchable as its base class.
try:
    raise copy.Error("base catch")
except Exception as exc:
    print("caught as Exception:", exc)  # -> caught as Exception: base catch
