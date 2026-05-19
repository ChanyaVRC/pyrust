# Issue #712: module-level and class-level type annotations must be stored in
# __annotations__.  The compiler was previously discarding annotation expressions
# entirely; this fixture exercises both module scope and class scope.

# ── Module scope ─────────────────────────────────────────────────────────────

# Annotated assignment: name appears in __annotations__, variable has value.
x: int = 42
assert x == 42

# Bare annotation (no value): name appears in __annotations__, no class attr.
y: str
z: list

print(sorted(__annotations__.keys()))    # ['x', 'y', 'z']
print(__annotations__['x'])             # <class 'int'>
print(__annotations__['y'])             # <class 'str'>
print(__annotations__['z'])             # <class 'list'>

# ── Function scope ────────────────────────────────────────────────────────────
# Annotations inside functions are NOT stored in __annotations__ at runtime
# (CPython never has).  Verify the module dict is not polluted.

def f():
    a: int = 10
    b: str = "hi"
    return a, b

assert f() == (10, "hi")

# Module-level __annotations__ must not contain 'a' or 'b'.
assert "a" not in __annotations__
assert "b" not in __annotations__

# ── Class scope ───────────────────────────────────────────────────────────────

class Point:
    x: int
    y: int = 0

print(sorted(Point.__annotations__.keys()))   # ['x', 'y']
print(Point.__annotations__['x'])             # <class 'int'>
print(Point.__annotations__['y'])             # <class 'int'>
assert not hasattr(Point, 'x')                # bare annotation — no attr
assert hasattr(Point, 'y')                    # annotated assignment — has attr
assert Point.y == 0

# Class with mixed annotations and plain assignments
class Mixed:
    a: int = 1
    b: str
    c = 99  # no annotation

print(sorted(Mixed.__annotations__.keys()))   # ['a', 'b']
assert Mixed.a == 1
assert not hasattr(Mixed, 'b')
assert Mixed.c == 99

# ── Annotation ordering ───────────────────────────────────────────────────────
# CPython preserves source order in __annotations__ (dict insertion order).

class Ordered:
    c: float = 3.0
    a: int = 1
    b: str = "x"

print(list(Ordered.__annotations__.keys()))   # ['c', 'a', 'b']

print("module annotations OK")
