"""A retained catch_warnings class remains constructible after re-import."""

import sys
import warnings as old_warnings


old_catch_warnings = old_warnings.catch_warnings
del sys.modules["warnings"]
del old_warnings

import warnings as new_warnings


context = old_catch_warnings(record=True)
with context as recorded:
    new_warnings.simplefilter("always")
    new_warnings.warn("retained generation")

message = recorded[0]
print(
    "retained catch:",
    type(context) is old_catch_warnings,
    len(recorded),
    type(message).__name__,
    type(message).__module__,
    str(message.message),
)
