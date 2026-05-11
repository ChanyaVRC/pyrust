class Vec2:
    def __init__(self, x, y):
        self.x = x
        self.y = y
    def __add__(self, other):
        if not isinstance(other, Vec2):
            return NotImplemented
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
        x_eq = self.x == other.x
        y_eq = self.y == other.y
        return x_eq and y_eq

a = Vec2(1, 2)
b = Vec2(3, 4)

c = a + b
assert c.x == 4 and c.y == 6, f"add failed: {c.x} {c.y}"

d = b - a
assert d.x == 2 and d.y == 2, f"sub failed"

e = a * 3
assert e.x == 3 and e.y == 6, f"mul failed"

f = 3 * a
assert f.x == 3 and f.y == 6, f"rmul failed"

g = -a
assert g.x == -1 and g.y == -2, f"neg failed"

assert a == Vec2(1, 2), "eq1 failed"
assert not (a == b), "ne failed"

# TypeError for unsupported op (NotImplemented returned from __add__)
try:
    result = a + 5
    assert False, "should raise TypeError"
except TypeError:
    pass

print("op overload OK")
