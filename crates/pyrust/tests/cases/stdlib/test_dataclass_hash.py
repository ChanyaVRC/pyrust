from dataclasses import dataclass, field

# frozen=True generates value-based __hash__
@dataclass(frozen=True)
class FrozenPoint:
    x: int
    y: int

fp1 = FrozenPoint(1, 2)
fp2 = FrozenPoint(1, 2)
fp3 = FrozenPoint(1, 3)

print(fp1 == fp2)              # True
print(hash(fp1) == hash(fp2))  # True
print(len({fp1, fp2}))         # 1 (same hash and equal)
print(len({fp1, fp3}))         # 2

# Regular dataclass with eq=True sets __hash__ = None (unhashable)
@dataclass
class Point:
    x: int
    y: int

p = Point(1, 2)
print(p == Point(1, 2))  # True
print(Point.__hash__ is None)  # True
try:
    hash(p)
except TypeError:
    print("unhashable")  # unhashable

# eq=False: hash is NOT suppressed (identity hash)
@dataclass(eq=False)
class NoEq:
    x: int

ne = NoEq(5)
print(hash(ne) == hash(ne))  # True (identity)

# unsafe_hash=True generates __hash__ even without frozen
@dataclass(unsafe_hash=True)
class UnsafeHashed:
    a: int
    b: int

u1 = UnsafeHashed(7, 8)
u2 = UnsafeHashed(7, 8)
print(hash(u1) == hash(u2))  # True

# Frozen hash respects compare=False fields (excluded from hash)
@dataclass(frozen=True)
class WithExcluded:
    x: int
    y: int = field(compare=False)

w1 = WithExcluded(1, 100)
w2 = WithExcluded(1, 200)
print(w1 == w2)               # True (y excluded from eq)
print(hash(w1) == hash(w2))   # True (y excluded from hash)

# Frozen hash matches the equivalent plain-tuple hash
print(hash(FrozenPoint(3, 4)) == hash((3, 4)))  # True

# unsafe_hash=True over an explicit __hash__ is an error (cannot overwrite).
try:
    @dataclass(unsafe_hash=True)
    class Clash:
        x: int
        def __hash__(self):
            return 1
    print("no error")
except TypeError:
    print("cannot overwrite")  # cannot overwrite

# field(hash=...) selects hash membership independently of compare.
@dataclass(frozen=True)
class FieldHash:
    a: int
    b: int = field(compare=False, hash=True)   # out of eq, in hash
    c: int = field(compare=True, hash=False)    # in eq, out of hash

h1 = FieldHash(1, 2, 3)
h2 = FieldHash(1, 2, 9)   # c differs (excluded from hash) → same hash
h3 = FieldHash(1, 8, 3)   # b differs (included in hash) → different hash
print(hash(h1) == hash(h2))   # True
print(hash(h1) == hash(h3))   # False
print(hash(h1) == hash((1, 2)))  # True: only a and b

# An explicit __hash__ survives the default eq=True/frozen=False case.
@dataclass
class KeepHash:
    x: int
    def __hash__(self):
        return 1234

print(hash(KeepHash(0)))  # 1234
