class Counter:
    def __init__(self, n):
        self.n = n

    def __iadd__(self, other):
        self.n += other
        return self

    def __isub__(self, other):
        self.n -= other
        return self

    def __imul__(self, other):
        self.n *= other
        return self


c = Counter(10)
c += 5
assert c.n == 15, c.n

c -= 3
assert c.n == 12, c.n

c *= 2
assert c.n == 24, c.n


# __iadd__ that returns NotImplemented falls back to __add__
class Vec2:
    def __init__(self, x, y):
        self.x = x
        self.y = y

    def __add__(self, other):
        return Vec2(self.x + other.x, self.y + other.y)

    def __iadd__(self, other):
        return NotImplemented


v = Vec2(1, 2)
v += Vec2(3, 4)
assert v.x == 4 and v.y == 6, (v.x, v.y)


# Augmented assignment on a class with no __iadd__ falls back to __add__
class Num:
    def __init__(self, n):
        self.n = n

    def __add__(self, other):
        return Num(self.n + other.n)


n = Num(5)
n += Num(3)
assert n.n == 8, n.n


print("inplace dunders OK")
