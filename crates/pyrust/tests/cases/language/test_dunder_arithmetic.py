class Vec2:
    def __init__(self, x, y):
        self.x = x
        self.y = y
    def __add__(self, other):
        return Vec2(self.x + other.x, self.y + other.y)
    def __sub__(self, other):
        return Vec2(self.x - other.x, self.y - other.y)
    def __mul__(self, scalar):
        return Vec2(self.x * scalar, self.y * scalar)
    def __rmul__(self, scalar):
        return Vec2(self.x * scalar, self.y * scalar)
    def __neg__(self):
        return Vec2(-self.x, -self.y)
    def __eq__(self, other):
        return self.x == other.x and self.y == other.y
    def __repr__(self):
        return "Vec2(" + str(self.x) + ", " + str(self.y) + ")"
    def __str__(self):
        return "(" + str(self.x) + ", " + str(self.y) + ")"

a = Vec2(1, 2)
b = Vec2(3, 4)
c = a + b
assert c.x == 4 and c.y == 6
d = b - a
assert d.x == 2 and d.y == 2
e = a * 3
assert e.x == 3 and e.y == 6
f = 3 * a
assert f.x == 3 and f.y == 6
neg_a = -a
assert neg_a == Vec2(-1, -2)
assert a == Vec2(1, 2)
assert a != b
assert str(a) == "(1, 2)"
assert repr(a) == "Vec2(1, 2)"

class Counter:
    def __init__(self, n):
        self.n = n
    def __add__(self, other):
        return Counter(self.n + other.n)
    def __lt__(self, other):
        return self.n < other.n
    def __le__(self, other):
        return self.n <= other.n
    def __gt__(self, other):
        return self.n > other.n
    def __ge__(self, other):
        return self.n >= other.n
    def __eq__(self, other):
        return self.n == other.n
    def __contains__(self, item):
        return item == self.n

x = Counter(3)
y = Counter(5)
assert (x + y).n == 8
assert x < y
assert y > x
assert x <= x
assert y >= y
assert not (x > y)
assert 3 in x
assert 5 not in x

# __radd__: when both __add__ and __radd__ are defined, int + obj uses __radd__
class Addable:
    def __init__(self, v):
        self.v = v
    def __add__(self, other):
        return Addable(self.v + other)
    def __radd__(self, other):
        return Addable(other + self.v)

a = Addable(10)
assert (a + 5).v == 15
result = 5 + a
assert result.v == 15

# No __eq__ defined: falls back to identity comparison
class Bare:
    pass

b1 = Bare()
b2 = Bare()
assert b1 == b1
assert b1 != b2

print("dunder arithmetic OK")
