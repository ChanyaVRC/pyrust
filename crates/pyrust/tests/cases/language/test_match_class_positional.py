"""
Positional sub-patterns in class patterns (PEP 634 §8).

`case ClassName(p0, p1, ...)` maps to `__match_args__` entries at runtime.
"""


class Point:
    __match_args__ = ("x", "y")

    def __init__(self, x, y):
        self.x = x
        self.y = y


class Color:
    __match_args__ = ("r", "g", "b")

    def __init__(self, r, g, b):
        self.r = r
        self.g = g
        self.b = b


# --- Basic positional matching ---

p = Point(3, 0)
match p:
    case Point(0, 0):
        print("origin")
    case Point(x, 0):
        print("x-axis", x)
    case _:
        print("other")
# x-axis 3

# Exact match (0, 0)
match Point(0, 0):
    case Point(0, 0):
        print("origin")
    case _:
        print("no")
# origin

# Capture both positional
match Point(7, 8):
    case Point(a, b):
        print("captured", a, b)
# captured 7 8

# Three-arg positional
match Color(255, 0, 128):
    case Color(r, 0, b):
        print("no-green", r, b)
    case _:
        print("other")
# no-green 255 128

# --- Mixed positional + keyword ---

match Point(0, 5):
    case Point(0, y=val):
        print("mixed", val)
    case _:
        print("no")
# mixed 5

# --- Keyword-only form still works (regression) ---

match Point(1, 2):
    case Point(x=1, y=2):
        print("kw match")
    case _:
        print("no")
# kw match

# --- isinstance check happens first ---

match "not a point":
    case Point(0, 0):
        print("wrong")
    case _:
        print("not a Point")
# not a Point

# --- TypeError: no __match_args__ ---

class NoArgs:
    def __init__(self):
        pass


try:
    match NoArgs():
        case NoArgs(1,):
            print("matched")
except TypeError as e:
    print("TypeError:", e)
# TypeError: NoArgs() accepts 0 positional sub-patterns (1 given)

# --- TypeError: __match_args__ too short ---

class Short:
    __match_args__ = ("x",)

    def __init__(self, x, y):
        self.x = x
        self.y = y


try:
    match Short(1, 2):
        case Short(1, 2):
            print("matched")
except TypeError as e:
    print("TypeError:", e)
# TypeError: Short() accepts 1 positional sub-pattern (2 given)

# --- Positional patterns in a guard ---

match Point(4, 0):
    case Point(x, 0) if x > 3:
        print("positive x-axis beyond 3:", x)
    case _:
        print("other")
# positive x-axis beyond 3: 4

# --- Nested class pattern ---

class Segment:
    __match_args__ = ("start", "end")

    def __init__(self, start, end):
        self.start = start
        self.end = end


seg = Segment(Point(0, 0), Point(1, 1))
match seg:
    case Segment(Point(0, 0), Point(ex, ey)):
        print("from origin to", ex, ey)
    case _:
        print("other")
# from origin to 1 1
