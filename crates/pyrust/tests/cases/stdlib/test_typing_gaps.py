# typing module — runtime helpers filled in for issue #2516:
# TYPE_CHECKING, get_type_hints, get_origin, get_args, NamedTuple,
# runtime_checkable, final, reveal_type, and the special-form markers.
#
# Output is kept CPython-stable: assertions compare identity / tuples and only
# print at the end, avoiding reprs that differ between runtimes.

from typing import (
    TYPE_CHECKING,
    get_type_hints,
    get_origin,
    get_args,
    List,
    Dict,
    Optional,
    Union,
    Type,
    NamedTuple,
    Protocol,
    runtime_checkable,
    final,
    no_type_check,
    assert_type,
    get_overloads,
)

# TYPE_CHECKING is always False at runtime.
assert TYPE_CHECKING is False
if TYPE_CHECKING:  # never taken at runtime
    raise AssertionError("TYPE_CHECKING block ran at runtime")

# get_type_hints on a function resolves its annotations.
def f(x: int, y: str) -> bool:
    pass


assert get_type_hints(f) == {"x": int, "y": str, "return": bool}

# A function with no annotations yields an empty dict.
def g(x, y):
    pass


assert get_type_hints(g) == {}

# get_type_hints on a class merges base annotations and resolves forward refs.
class Base:
    a: int


class Derived(Base):
    b: "str"


assert get_type_hints(Derived) == {"a": int, "b": str}

# get_origin / get_args over PEP 585 / typing generics.
assert get_origin(List[int]) is list
assert get_args(List[int]) == (int,)
assert get_origin(Dict[str, int]) is dict
assert get_args(Dict[str, int]) == (str, int)
assert get_origin(int) is None
assert get_args(int) == ()

# Optional[X] is Union[X, None]: args include NoneType.
assert get_args(Optional[str]) == (str, type(None))
assert get_args(Union[int, str]) == (int, str)
# Optional and Union share the same normalised origin.
assert get_origin(Optional[str]) is get_origin(Union[int, str])

# Type[int] origin is the `type` builtin.
assert get_origin(Type[int]) is type

# NamedTuple class form: positional construction, fields, defaults, methods.
class Point(NamedTuple):
    x: int
    y: int = 0

    def total(self):
        return self.x + self.y


p = Point(1, 2)
assert p.x == 1 and p.y == 2
assert p._fields == ("x", "y")
assert tuple(p) == (1, 2)
assert p.total() == 3
# Default applies for the second field.
assert Point(5) == (5, 0)

# NamedTuple functional form (list-of-pairs and keyword variants).
P2 = NamedTuple("P2", [("a", int), ("b", str)])
assert P2(1, "z")._fields == ("a", "b")
P3 = NamedTuple("P3", a=int, b=int)
assert P3(1, 2) == (1, 2)

# Decorators / markers are runtime no-ops that return their argument.
@final
class FinalClass:
    pass


@runtime_checkable
class MyProto(Protocol):
    pass


@no_type_check
def annotated(x: int) -> int:
    return x


assert annotated(7) == 7
assert assert_type(42, int) == 42
assert get_overloads(f) == []

print("typing gaps ok")
