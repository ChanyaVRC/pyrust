# typing module — TypeVar, Generic, Protocol, cast, overload stubs.
# Verifies the names added in #1349 are importable and behave correctly
# at runtime (pyrust does not perform static type checking).

from typing import TypeVar, Generic, Protocol, cast, overload

# ── TypeVar ──────────────────────────────────────────────────────────────────

T = TypeVar('T')
assert T.__name__ == 'T', f"Expected T.__name__ == 'T', got {T.__name__!r}"

S = TypeVar('S')
assert S.__name__ == 'S'

# TypeVar instances are not the same object
assert T is not S

# __constraints__ and __bound__ exist and have expected defaults
assert T.__constraints__ == ()
assert T.__bound__ is None

# ── Generic ──────────────────────────────────────────────────────────────────

# Generic[T] must not raise (used as a class base)
class Stack(Generic[T]):
    pass

s = Stack()
assert s is not None

# Multiple type params
K = TypeVar('K')
V = TypeVar('V')

class Mapping(Generic[K, V]):
    pass

m = Mapping()
assert m is not None

# ── Protocol ─────────────────────────────────────────────────────────────────

class Drawable(Protocol):
    def draw(self) -> None: ...

class Circle:
    def draw(self) -> None:
        pass

c = Circle()
c.draw()

# ── cast ─────────────────────────────────────────────────────────────────────

# cast(typ, val) returns val unchanged at runtime
result = cast(int, "hello")
assert result == "hello", f"Expected 'hello', got {result!r}"
assert type(result) is str

result2 = cast(str, 42)
assert result2 == 42

# ── overload ─────────────────────────────────────────────────────────────────

# @overload is a no-op decorator; only the last plain definition is called

@overload
def greet(x: int) -> str: ...

@overload
def greet(x: str) -> str: ...

def greet(x):
    return f"hello {x}"

assert greet(1) == "hello 1"
assert greet("world") == "hello world"

# ── import also works for legacy names still present ─────────────────────────

from typing import Any, Optional, Union, List, Dict

assert Any is not None
opt = Optional[int]
assert opt is not None
u = Union[int, str]
assert u is not None
assert List[int] is not None
assert Dict[str, int] is not None

print("typing module ok")
