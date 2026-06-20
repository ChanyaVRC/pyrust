# TypedDict rejects isinstance/issubclass checks (PEP 589, issue #2738).
# CPython's `_TypedDictMeta.__subclasscheck__` / `__instancecheck__` both raise
# `TypeError: TypedDict does not support instance and class checks`.
from typing import TypedDict


class Base(TypedDict):
    x: int


class Child(Base):
    y: str


# functional form builds the same metaclass.
Movie = TypedDict("Movie", {"title": str})


checks = [
    ("issubclass(Child, Base)", lambda: issubclass(Child, Base)),
    ("issubclass(Base, Base)", lambda: issubclass(Base, Base)),
    ("issubclass(dict, Base)", lambda: issubclass(dict, Base)),
    ("issubclass(Movie, Base)", lambda: issubclass(Movie, Base)),
    ("isinstance({}, Child)", lambda: isinstance({}, Child)),
    ("isinstance(Base(x=1), Base)", lambda: isinstance(Base(x=1), Base)),
    ("isinstance({}, Movie)", lambda: isinstance({}, Movie)),
]

for label, fn in checks:
    try:
        print(label, "->", fn())
    except TypeError as e:
        print(label, "-> TypeError:", e)


# A TypedDict as the *first* arg uses `dict.__subclasscheck__`, so this is fine:
# the runtime class is a plain dict subclass.
print("issubclass(Base, dict) ->", issubclass(Base, dict))

# Instances of a TypedDict are plain dicts, so isinstance against `dict` works.
print("isinstance(Base(x=1), dict) ->", isinstance(Base(x=1), dict))
