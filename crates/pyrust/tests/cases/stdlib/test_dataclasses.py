# `dataclasses` module parity (issue #2610).  Exercises the `@dataclass`
# decorator (generated __init__ / __repr__ / __eq__), defaults, frozen
# instances, field() with default_factory, inheritance ordering, and the
# fields / asdict / astuple / replace / is_dataclass helpers.
from dataclasses import (dataclass, field, fields, asdict, astuple, replace,
                         is_dataclass)


@dataclass
class Point:
    x: float
    y: float
    z: float = 0.0


p = Point(1.0, 2.0)
print(p)
print(p.x, p.y, p.z)
print(asdict(p))
print(astuple(p))
print([(f.name, f.type) for f in fields(p)])

# Generated __eq__.
print(Point(1.0, 2.0) == Point(1.0, 2.0))
print(Point(1.0, 2.0) == Point(1.0, 9.0))
print(Point(1.0, 2.0) == "not a point")

# is_dataclass on class and instance.
print(is_dataclass(Point), is_dataclass(p), is_dataclass(42))

# replace returns a new instance with overrides.
q = replace(p, y=5.0)
print(q, p)


# Frozen instances reject attribute assignment.
@dataclass(frozen=True)
class Immutable:
    x: int
    y: int


im = Immutable(1, 2)
print(im, im.x, im.y)
try:
    im.x = 3
except (AttributeError, TypeError):
    print("immutable")
print(Immutable(1, 2) == Immutable(1, 2))


# field(default_factory=...) gives each instance its own mutable default.
@dataclass
class Bag:
    items: list = field(default_factory=list)
    label: str = "bag"


b1 = Bag()
b2 = Bag()
b1.items.append("x")
print(b1.items, b2.items, b1.label)


# Inheritance: base fields precede derived fields in order.
@dataclass
class Base:
    a: int
    b: int = 0


@dataclass
class Derived(Base):
    c: int = 5


d = Derived(1, 2, 3)
print(d)
print([f.name for f in fields(d)])
print(asdict(d))


# Nested dataclasses are converted recursively by asdict / astuple.
@dataclass
class Line:
    start: Point
    end: Point


line = Line(Point(0.0, 0.0), Point(1.0, 1.0))
print(asdict(line))
print(astuple(line))
