# Issue #2745: typing.Any / TypeVar / TypedDict expose __module__ == 'typing'.
import typing
from typing import Any, TypeVar, TypedDict

# __module__ on the three objects.
print(typing.Any.__module__)
print(typing.TypeVar.__module__)
print(typing.TypedDict.__module__)

# Same via direct imports (identity preserved).
print(Any.__module__)
print(TypeVar.__module__)
print(TypedDict.__module__)

# Reprs follow from __module__ being correct.
print(repr(typing.Any))
print(repr(typing.TypeVar))

# Regression guard: ordinary method descriptors still have no __module__,
# and top-level builtins keep __module__ == 'builtins'.
try:
    str.upper.__module__
except AttributeError as e:
    print("AttributeError:", e)
print(len.__module__)

# TypeVar instances keep their __name__.
T = TypeVar("T")
print(T.__name__)

# Regression guard: a TypeVar *instance* created at top level reports the
# caller's module (__main__), NOT the class's 'typing' — seeding __module__ on
# the class must not leak onto instances.
print(T.__module__)
print(TypeVar("TC", int, str).__module__)
print(TypeVar("TB", bound=int).__module__)

# __name__ / __qualname__ parity for the three objects (issue #2745).
print(typing.Any.__name__)
print(typing.Any.__qualname__)
print(typing.TypeVar.__name__)
print(typing.TypeVar.__qualname__)
print(typing.TypedDict.__name__)
print(typing.TypedDict.__qualname__)
