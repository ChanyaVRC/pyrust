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


# Other arithmetic in-place dunders
class Arith:
    def __init__(self, n):
        self.n = n
    def __ifloordiv__(self, other):
        self.n //= other
        return self
    def __imod__(self, other):
        self.n %= other
        return self
    def __ipow__(self, other):
        self.n **= other
        return self

a = Arith(17)
a //= 3
assert a.n == 5, a.n

a = Arith(17)
a %= 5
assert a.n == 2, a.n

a = Arith(3)
a **= 4
assert a.n == 81, a.n


# Bitwise in-place dunders
class Bits:
    def __init__(self, n):
        self.n = n
    def __ior__(self, other):
        self.n |= other
        return self
    def __iand__(self, other):
        self.n &= other
        return self
    def __ixor__(self, other):
        self.n ^= other
        return self
    def __ilshift__(self, other):
        self.n <<= other
        return self
    def __irshift__(self, other):
        self.n >>= other
        return self

b = Bits(0b1010)
b |= 0b0101
assert b.n == 0b1111, b.n

b = Bits(0b1111)
b &= 0b1010
assert b.n == 0b1010, b.n

b = Bits(0b1010)
b ^= 0b1100
assert b.n == 0b0110, b.n

b = Bits(1)
b <<= 3
assert b.n == 8, b.n

b = Bits(16)
b >>= 2
assert b.n == 4, b.n


print("inplace dunders OK")
