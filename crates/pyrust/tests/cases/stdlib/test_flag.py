# enum — Flag / IntFlag / StrEnum surface (issue #2763).
#
# Bitwise composition, membership / `__contains__`, falsy empty flag, repr/str
# of single and composite members, value lookup of composites, IntFlag int
# compatibility (and plain-int operands), and StrEnum string compatibility —
# all compared against CPython 3.12 by the parity harness.

from enum import Flag, IntFlag, StrEnum, auto


class Color(Flag):
    RED = auto()
    GREEN = auto()
    BLUE = auto()
    WHITE = 7  # explicit named composite (auto-based in-body composite unsupported)


# ── single-member repr / str / value ─────────────────────────────────────────
print(repr(Color.RED))            # <Color.RED: 1>
print(str(Color.RED))             # Color.RED
print(Color.RED.value, Color.GREEN.value, Color.BLUE.value)  # 1 2 4

# ── bitwise composition ──────────────────────────────────────────────────────
print(str(Color.RED | Color.GREEN))    # Color.RED|GREEN
print(repr(Color.RED | Color.GREEN))   # <Color.RED|GREEN: 3>
print(repr(Color.GREEN | Color.BLUE))  # <Color.GREEN|BLUE: 6>

# ── named composite ──────────────────────────────────────────────────────────
print(str(Color.WHITE))           # Color.WHITE
print(repr(Color.WHITE))          # <Color.WHITE: 7>
print(Color.WHITE.value)          # 7

# ── falsy empty flag ─────────────────────────────────────────────────────────
print(bool(Color.RED & Color.BLUE))   # False
print(bool(Color.RED))                # True
print(repr(Color.RED & Color.BLUE))   # <Color: 0>
print(str(Color.RED & Color.BLUE))    # Color(0)

# ── membership / __contains__ ────────────────────────────────────────────────
print(Color.RED in Color.WHITE)            # True
composite = Color.RED | Color.GREEN
print(Color.RED in composite)              # True
print(Color.BLUE in composite)             # False

# ── composite .name ──────────────────────────────────────────────────────────
print(Color.RED.name)             # RED
print((Color.RED | Color.GREEN).name)  # RED|GREEN
print(repr((Color.RED & Color.BLUE).name))  # None

# ── invert / xor ─────────────────────────────────────────────────────────────
print(repr(~Color.RED))           # <Color.GREEN|BLUE: 6>
print(repr(Color.RED ^ Color.WHITE))  # <Color.GREEN|BLUE: 6>

# ── iteration excludes composites ────────────────────────────────────────────
print(list(Color))                # [<Color.RED: 1>, <Color.GREEN: 2>, <Color.BLUE: 4>]

# ── value lookup of composites ───────────────────────────────────────────────
print(repr(Color(3)))             # <Color.RED|GREEN: 3>
print(repr(Color(0)))             # <Color: 0>


class WithZero(Flag):
    NONE = 0
    A = 1
    B = 2


# A named all-zero flag is accessible but not iterated (like CPython).
print(repr(WithZero.NONE))        # <WithZero.NONE: 0>
print(str(WithZero.NONE))         # WithZero.NONE
print(bool(WithZero.NONE))        # False
print(WithZero(0) is WithZero.NONE)  # True
print(list(WithZero))             # [<WithZero.A: 1>, <WithZero.B: 2>]


class Permission(IntFlag):
    READ = 1
    WRITE = 2
    EXECUTE = 4


p = Permission.READ | Permission.WRITE
print(p)                          # 3
print(repr(p))                    # <Permission.READ|WRITE: 3>
print(p.name)                     # READ|WRITE
print(int(p))                     # 3
print(p & Permission.READ)        # 1
print(Permission.READ == 1)       # True
print(Permission.READ | 2)        # 3
print(2 | Permission.READ)        # 3
print(Permission.READ in p)       # True

# ── IntFlag negative values mask into the named-bit range (two's complement) ──
print(int(Permission(-1)))        # 7
print(repr(Permission(-1)))       # <Permission.READ|WRITE|EXECUTE: 7>
print(int(Permission(-2)))        # 6
print(repr(Permission(-2)))       # <Permission.WRITE|EXECUTE: 6>


class Direction(StrEnum):
    NORTH = "north"
    SOUTH = "south"


print(Direction.NORTH == "north")     # True
print(str(Direction.NORTH))           # north
print(repr(Direction.NORTH))          # <Direction.NORTH: 'north'>
print(Direction("north") is Direction.NORTH)  # True
print("%s" % Direction.NORTH)         # north
print(f"{Direction.NORTH}")           # north


# ── StrEnum auto() lowercases the member name ────────────────────────────────
class Side(StrEnum):
    LEFT = auto()
    RIGHT = auto()
    BACK = "rear"


print(Side.LEFT.value, Side.RIGHT.value, Side.BACK.value)  # left right rear
print(repr(Side.LEFT))                # <Side.LEFT: 'left'>
print(Side("left") is Side.LEFT)      # True
