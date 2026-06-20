# TypedDict class syntax and functional form (PEP 589, issue #2718).
from typing import TypedDict


# ── class form ────────────────────────────────────────────────────────────
class Movie(TypedDict):
    title: str
    year: int


# A TypedDict is an annotation type: its instances are plain dicts.
m = {"title": "Blade Runner", "year": 1982}
print(type(m).__name__)
print(m["title"])

# Annotations are recorded on the class.
print(Movie.__annotations__)

# Calling the class builds a plain dict (CPython parity).
mv = Movie(title="X", year=2000)
print(type(mv).__name__, mv)

# Keys are required by default; required/optional/total bookkeeping is present.
print(sorted(Movie.__required_keys__))
print(sorted(Movie.__optional_keys__))
print(Movie.__total__)


# ── empty TypedDict ───────────────────────────────────────────────────────
class Empty(TypedDict):
    pass


print(Empty.__annotations__, Empty.__total__)
print(type(Empty()).__name__, Empty())


# ── total=False class keyword ──────────────────────────────────────────────
class Partial(TypedDict, total=False):
    a: int
    b: str


print(Partial.__annotations__)
print(Partial.__total__)
print(sorted(Partial.__required_keys__))
print(sorted(Partial.__optional_keys__))


# ── functional form ────────────────────────────────────────────────────────
Point = TypedDict("Point", {"x": int, "y": int})
print(Point.__annotations__)
print(Point.__total__)
print(sorted(Point.__required_keys__))
p = Point(x=1, y=2)
print(type(p).__name__, p["x"], p["y"])

# total=False functional form.
Q = TypedDict("Q", {"a": str}, total=False)
print(Q.__total__, sorted(Q.__optional_keys__))

# Providing both a dict and keywords is an error.
try:
    TypedDict("Bad", {"a": int}, b=str)
except TypeError as e:
    print("TypeError:", e)


# ── instances behave as ordinary dicts ─────────────────────────────────────
d = Movie(title="Alien", year=1979)
d["year"] += 1
print(d)
print(len(d), "title" in d)
