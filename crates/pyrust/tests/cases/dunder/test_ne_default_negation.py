# Issue #2645: a class defining __eq__ but not __ne__ derives __ne__ as the
# logical negation of __eq__ (CPython's slot_tp_richcompare), instead of the
# inherited identity-based object.__ne__.


# __eq__ returns True -> != is always False, even for the same object.
class AlwaysEq:
    def __eq__(self, other):
        return True


a = AlwaysEq()
print(a != a)
print(a != AlwaysEq())


# __eq__ returns False -> != is always True, including a != a.
class NeverEq:
    def __eq__(self, other):
        return False


b = NeverEq()
print(b != b)
print(b != NeverEq())


# An explicit __ne__ wins over the derived negation.
class ExplicitNe:
    def __eq__(self, other):
        return True

    def __ne__(self, other):
        return True


c = ExplicitNe()
print(c != c)


# An explicit __ne__ may return a non-bool value, returned as-is.
class RawNe:
    def __eq__(self, other):
        return True

    def __ne__(self, other):
        return "raw"


print(RawNe() != RawNe())


# NotImplemented from __eq__ falls back to the reflected/identity chain.
class NI:
    def __eq__(self, other):
        if not isinstance(other, NI):
            return NotImplemented
        return True


n = NI()
print(n != n)
print(n != 42)


# A non-bool __eq__ result is negated by truthiness (not bool(__eq__())).
class TruthyEq:
    def __eq__(self, other):
        return 5  # truthy


class FalsyEq:
    def __eq__(self, other):
        return 0  # falsy


print(TruthyEq() != TruthyEq())
print(FalsyEq() != FalsyEq())


# A subclass inherits the derived __ne__ from its base's __eq__.
class Base:
    def __eq__(self, other):
        return True


class Sub(Base):
    pass


print(Sub() != Sub())


# Reflected __eq__: left returns NotImplemented, right decides.
class L:
    def __eq__(self, other):
        return NotImplemented


class R:
    def __eq__(self, other):
        return True


print(L() != R())


# Container of instances compares element-wise (issue #436 path still works).
class Cell:
    def __init__(self, v):
        self.v = v

    def __eq__(self, other):
        return self.v == other.v


print([Cell(1)] != [Cell(1)])
print([Cell(1)] != [Cell(2)])

# Primitives are unaffected.
print(1 != 2, 1 != 1, "a" != "b", [1, 2] != [1, 2])
