# enum — Enum / IntEnum / auto core surface (issue #2611).
#
# Member access, .name/.value, identity comparison, iteration in definition
# order, repr/str, value lookup `Color(v)`, name lookup `Color[n]`, membership,
# len, aliases, and IntEnum int-compatibility — all compared against CPython
# 3.12 by the parity harness.

from enum import Enum, IntEnum, auto


class Color(Enum):
    RED = 1
    GREEN = 2
    BLUE = 3


# ── member access / attributes ───────────────────────────────────────────────
print(Color.RED)                  # Color.RED
print(Color.RED.name)             # RED
print(Color.RED.value)            # 1
print(repr(Color.RED))            # <Color.RED: 1>

# ── identity-based comparison ────────────────────────────────────────────────
print(Color.RED == Color.RED)     # True
print(Color.RED == Color.GREEN)   # False
print(Color.RED is Color.RED)     # True

# ── iteration in definition order ────────────────────────────────────────────
print(list(Color))                # [<Color.RED: 1>, <Color.GREEN: 2>, <Color.BLUE: 3>]
print([m.name for m in Color])    # ['RED', 'GREEN', 'BLUE']
print(len(Color))                 # 3

# ── value / name lookup ──────────────────────────────────────────────────────
print(Color(1))                   # Color.RED
print(Color(2))                   # Color.GREEN
print(Color['RED'])               # Color.RED
print(Color['BLUE'])              # Color.BLUE

# ── membership ───────────────────────────────────────────────────────────────
print(Color.RED in Color)         # True
print(1 in Color)                 # True
print(99 in Color)                # False

# ── invalid lookups raise ────────────────────────────────────────────────────
try:
    Color(99)
except ValueError as e:
    print("ValueError:", e)       # ValueError: 99 is not a valid Color
try:
    Color['NOPE']
except KeyError as e:
    print("KeyError:", e)         # KeyError: 'NOPE'

# ── type / isinstance ────────────────────────────────────────────────────────
print(type(Color.RED).__name__)   # Color
print(isinstance(Color.RED, Color))  # True

# ── enums are hashable / usable as dict keys ─────────────────────────────────
d = {Color.RED: "r", Color.GREEN: "g"}
print(d[Color.RED])               # r
print(Color.GREEN in d)           # True


# ── auto() ───────────────────────────────────────────────────────────────────
class Direction(Enum):
    NORTH = auto()
    SOUTH = auto()
    EAST = auto()


print(Direction.NORTH.value)      # 1
print(Direction.SOUTH.value)      # 2
print(Direction.EAST.value)       # 3


# auto() mixed with explicit values
class Mix(Enum):
    A = auto()
    B = 5
    C = auto()


print([(m.name, m.value) for m in Mix])  # [('A', 1), ('B', 5), ('C', 6)]


# ── aliases (duplicate value) ────────────────────────────────────────────────
class Shape(Enum):
    SQUARE = 1
    DIAMOND = 1
    CIRCLE = 2


print(Shape.SQUARE is Shape.DIAMOND)   # True
print(list(Shape))                     # [<Shape.SQUARE: 1>, <Shape.CIRCLE: 2>]
print(len(Shape))                      # 2
print(Shape['DIAMOND'])                # Shape.SQUARE


# ── IntEnum ──────────────────────────────────────────────────────────────────
class Status(IntEnum):
    OK = 200
    NOT_FOUND = 404


print(Status.OK == 200)           # True
print(Status.OK + 1)              # 201
print(Status.OK < Status.NOT_FOUND)  # True
print(int(Status.OK))             # 200
print(isinstance(Status.OK, int))    # True
print(str(Status.OK))             # 200
print(repr(Status.OK))            # <Status.OK: 200>
print(f"{Status.OK}")             # 200
print(f"{Status.OK!r}")           # <Status.OK: 200>
print(sorted([Status.NOT_FOUND, Status.OK]))  # [<Status.OK: 200>, <Status.NOT_FOUND: 404>]

# `int` precedes the enum base in the MRO so int behaviour (arithmetic,
# ordering) is inherited; the user enum base follows.  (CPython interposes a
# `ReprEnum` between `int` and `Enum` — an implementation detail pyrust does not
# model — so only the leading int-before-enum order is asserted here.)
mro = [c.__name__ for c in Status.__mro__]
print(mro.index("int") < mro.index("Enum"))  # True
print(mro[0], mro[1], mro[-1])               # Status IntEnum object
