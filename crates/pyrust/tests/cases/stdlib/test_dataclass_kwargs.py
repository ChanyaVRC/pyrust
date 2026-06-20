from dataclasses import dataclass, field


# order=True generates comparison methods
@dataclass(order=True)
class Score:
    value: int = 0


s1, s2 = Score(1), Score(2)
print(s1 < s2)   # True
print(s2 > s1)   # True
print(s1 <= s1)  # True
print(s1 >= s1)  # True
print(Score(3) > Score(1))  # True


# order with multiple fields compares lexicographically
@dataclass(order=True)
class Pair:
    a: int
    b: int


print(Pair(1, 2) < Pair(1, 3))  # True
print(Pair(1, 2) < Pair(2, 0))  # True
print(Pair(2, 0) < Pair(1, 9))  # False


# match_args=True (default) sets __match_args__
@dataclass
class Point:
    x: int
    y: int


print(Point.__match_args__)  # ('x', 'y')


# __match_args__ drives positional class patterns
match Point(1, 2):
    case Point(a, b):
        print(a, b)  # 1 2


# match_args=False omits it
@dataclass(match_args=False)
class NoMatch:
    x: int


print(hasattr(NoMatch, '__match_args__'))  # False


# kw_only=True makes all fields keyword-only
@dataclass(kw_only=True)
class KwOnly:
    a: int
    b: int


obj = KwOnly(a=1, b=2)
print(obj.a, obj.b)  # 1 2
try:
    KwOnly(1, 2)
except TypeError:
    print("TypeError: kw_only positional rejected")


# unsafe_hash with eq=True produces a usable __hash__
@dataclass(unsafe_hash=True)
class UH:
    a: int


u1, u2 = UH(5), UH(5)
print(hash(u1) == hash(u2))  # True


# default eq=True sets __hash__ to None (unhashable)
@dataclass
class Plain:
    a: int


print(Plain.__hash__ is None)  # True


# frozen=True makes instances hashable
@dataclass(frozen=True)
class Frozen:
    a: int


f1, f2 = Frozen(7), Frozen(7)
print(hash(f1) == hash(f2))  # True


# order=True with eq=False raises ValueError
try:
    @dataclass(order=True, eq=False)
    class Bad:
        x: int
except ValueError:
    print("ValueError: order without eq")


# eq=False leaves __hash__ inherited from object (instances stay hashable)
@dataclass(eq=False)
class NoEq:
    a: int


print(NoEq.__hash__ is not None)  # True


# slots=True returns a new class with __slots__ and no instance __dict__
@dataclass(slots=True)
class Slotted:
    x: int
    y: int = 5


sl = Slotted(1)
print(Slotted.__slots__)            # ('x', 'y')
print(hasattr(sl, "__dict__"))      # False
print(sl.x, sl.y)                   # 1 5

# field(kw_only=False) overrides a class-level kw_only=True
@dataclass(kw_only=True)
class Mixed:
    a: int = field(kw_only=False)
    b: int


print(Mixed(1, b=2).a, Mixed(1, b=2).b)  # 1 2
print(Mixed.__match_args__)               # ('a',)

# field(hash=False) drops a field from __hash__ but keeps it in __eq__
@dataclass(frozen=True)
class HashField:
    a: int
    b: int = field(hash=False)


print(hash(HashField(1, 2)) == hash(HashField(1, 9)))  # True
print(HashField(1, 2) == HashField(1, 9))              # False
