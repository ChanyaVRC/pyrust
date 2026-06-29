from dataclasses import dataclass, field, KW_ONLY


# __post_init__ runs at the end of the generated __init__.
@dataclass
class Point:
    x: float
    y: float

    def __post_init__(self):
        self.norm = (self.x**2 + self.y**2) ** 0.5


p = Point(3.0, 4.0)
print(p.norm)


# A class without __post_init__ still constructs fine.
@dataclass
class Plain:
    a: int
    b: int = 2


print(Plain(1))
print(Plain(1, 5))


# KW_ONLY makes every following field keyword-only.
@dataclass
class Order:
    item: str
    _: KW_ONLY
    count: int = 1
    price: float = 0.0


o = Order("apple", count=3, price=1.5)
print(o)
print(repr(o))
print(o.count, o.price)


# A positional argument for a kw-only field is a TypeError.
try:
    Order("banana", 2)
except TypeError as e:
    print(type(e).__name__)


# __post_init__ combined with KW_ONLY.
@dataclass
class Vec:
    x: int
    _: KW_ONLY
    y: int = 0

    def __post_init__(self):
        self.total = self.x + self.y


v = Vec(10, y=5)
print(v.total)


# Specifying KW_ONLY twice in one class body is a TypeError.
try:

    @dataclass
    class Bad:
        a: int
        _: KW_ONLY
        b: int
        __: KW_ONLY
        c: int

except TypeError:
    print("kw_only twice rejected")
