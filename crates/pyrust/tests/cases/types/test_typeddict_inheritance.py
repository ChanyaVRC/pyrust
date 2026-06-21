# TypedDict inheritance merges ancestor fields (PEP 589, issue #2732).
from typing import TypedDict, get_type_hints


# ── single inheritance ──────────────────────────────────────────────────────
class Base(TypedDict):
    x: int


class Child(Base):
    y: str


# __annotations__ includes inherited fields, in MRO order (ancestors first).
print(list(Child.__annotations__))
print(sorted(Child.__required_keys__))
print(sorted(Child.__optional_keys__))


# ── transitive inheritance ──────────────────────────────────────────────────
class GrandChild(Child):
    z: float


print(list(GrandChild.__annotations__))
print(sorted(GrandChild.__required_keys__))


# ── total=False mixing ──────────────────────────────────────────────────────
class A(TypedDict):
    a: int


class B(A, total=False):
    b: str


print(sorted(B.__required_keys__))
print(sorted(B.__optional_keys__))
print(B.__total__)


# A required field added on top of an optional-field base stays required.
class C(B):
    c: float


print(sorted(C.__required_keys__))
print(sorted(C.__optional_keys__))


# ── multiple inheritance ────────────────────────────────────────────────────
class D(TypedDict):
    d: int


class E(A, D):
    e: int


print(sorted(E.__annotations__))
print(sorted(E.__required_keys__))


# A key required in one base and optional in another resolves by *last* base
# (each base moves the key between the required/optional sets); the two sets
# must stay disjoint.
class Req(TypedDict):
    k: int


class Opt(TypedDict, total=False):
    k: int


class MIa(Req, Opt):
    pass


class MIb(Opt, Req):
    pass


print(sorted(MIa.__required_keys__), sorted(MIa.__optional_keys__))
print(sorted(MIb.__required_keys__), sorted(MIb.__optional_keys__))


# ── field override ──────────────────────────────────────────────────────────
class F(A):
    a: str


print(F.__annotations__["a"].__name__)


# ── get_type_hints sees the merged annotations ──────────────────────────────
print(sorted(get_type_hints(C)))


# ── empty subclass body ─────────────────────────────────────────────────────
class Empty(A):
    pass


print(sorted(Empty.__annotations__))
print(sorted(Empty.__required_keys__))
print(Empty.__total__)


# ── instances of a derived TypedDict are still plain dicts ──────────────────
m = C(a=1, b="x", c=2.0)
print(type(m).__name__)
print(sorted(m.items()))
