class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def __eq__(self, other):
        return self.x == other.x and self.y == other.y

    def __ne__(self, other):
        return not self.__eq__(other)

    def __lt__(self, other):
        if self.x != other.x:
            return self.x < other.x
        return self.y < other.y

    def __le__(self, other):
        return self == other or self < other

    def __gt__(self, other):
        return other < self

    def __ge__(self, other):
        return other <= self

    def __str__(self):
        return "Point(" + str(self.x) + ", " + str(self.y) + ")"

    def __repr__(self):
        return "Point(x=" + repr(self.x) + ", y=" + repr(self.y) + ")"


a = Point(1, 2)
b = Point(3, 4)
c = Point(1, 2)

# __eq__ and __ne__
assert a == c, "eq failed"
assert a != b, "ne failed"
assert not (a == b), "ne2 failed"

# __lt__, __le__, __gt__, __ge__
assert a < b, "lt failed"
assert a <= b, "le1 failed"
assert a <= c, "le2 failed"
assert b > a, "gt failed"
assert b >= a, "ge1 failed"
assert a >= c, "ge2 failed"

# __str__
assert str(a) == "Point(1, 2)", repr(str(a))
assert str(b) == "Point(3, 4)", repr(str(b))

# __repr__
assert repr(a) == "Point(x=1, y=2)", repr(repr(a))


# Class with __repr__ only — str() should fall back to __repr__
class Named:
    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return "Named(" + repr(self.name) + ")"


n = Named("hello")
assert repr(n) == "Named('hello')", repr(repr(n))
assert str(n) == "Named('hello')", repr(str(n))


# NotImplemented from __eq__ falls back to identity comparison
class Weird:
    def __eq__(self, other):
        return NotImplemented


w1 = Weird()
w2 = Weird()
assert not (w1 == w2), "identity fallback failed"
assert w1 == w1, "identity self failed"

print("cmp_str OK")
