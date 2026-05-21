# typing module — runtime stubs for annotation-only imports.
# pyrust does not do static type checking; these verify the names are
# importable and work at the syntactic level (subscripting, identity, etc.).

from typing import (
    List, Dict, Set, Tuple,
    Optional, Union, Callable,
    ClassVar, Final, Literal, Type,
    Any,
)

# Aliases for primitive types: List[int] works like list[int] (PEP 585)
assert List[int] is not None
assert Dict[str, int] is not None
assert Set[float] is not None
assert Tuple[int, str] is not None
assert Type[int] is not None

# Optional[X] is Union[X, None] — just needs to be subscriptable
opt = Optional[int]
assert opt is not None

# Union is subscriptable
u = Union[int, str]
assert u is not None

# Callable is subscriptable
cb = Callable[[int], str]
assert cb is not None

# ClassVar, Final, Literal
cv = ClassVar[int]
fn = Final[str]
lit = Literal[42]
assert cv is not None
assert fn is not None
assert lit is not None

# Any is a usable sentinel
assert Any is not None
# Any as annotation (no runtime effect)
x: Any = 42
assert x == 42

# Annotated assignment with typing names works
items: List[int] = [1, 2, 3]
assert items == [1, 2, 3]

mapping: Dict[str, int] = {"a": 1}
assert mapping["a"] == 1

print("typing ok")
