# Issue #2639: set/dict subclass in-place operators (|=, &=, -=, ^=) whose
# user override returns NotImplemented must fall back to plain binary op,
# yielding a plain set/dict (the subclass type is NOT preserved on fallback).
# A subclass without an override keeps the subclass type and object identity.


# --- set: __ior__ returning NotImplemented falls back to plain set ---
class SOr(set):
    def __ior__(self, other):
        return NotImplemented


s = SOr({1})
s |= {2}
print(type(s).__name__, sorted(s))


# --- set: __iand__ ---
class SAnd(set):
    def __iand__(self, other):
        return NotImplemented


s2 = SAnd({1, 2})
s2 &= {1}
print(type(s2).__name__, sorted(s2))


# --- set: __isub__ ---
class SSub(set):
    def __isub__(self, other):
        return NotImplemented


s3 = SSub({1, 2})
s3 -= {1}
print(type(s3).__name__, sorted(s3))


# --- set: __ixor__ ---
class SXor(set):
    def __ixor__(self, other):
        return NotImplemented


s5 = SXor({1, 2})
s5 ^= {2, 3}
print(type(s5).__name__, sorted(s5))


# --- dict: __ior__ ---
class DOr(dict):
    def __ior__(self, other):
        return NotImplemented


d = DOr({"a": 1})
d |= {"b": 2}
print(type(d).__name__, sorted(d.items()))


# --- no override: subclass type AND object identity preserved (in-place) ---
class SPlain(set):
    pass


s4 = SPlain({1})
before = id(s4)
s4 |= {2}
print(type(s4).__name__, sorted(s4), id(s4) == before)


class DPlain(dict):
    pass


d2 = DPlain({"a": 1})
before_d = id(d2)
d2 |= {"b": 2}
print(type(d2).__name__, sorted(d2.items()), id(d2) == before_d)


# --- NotImplemented fallback creates a NEW plain object (identity changes) ---
class SId(set):
    def __ior__(self, other):
        return NotImplemented


s6 = SId({1})
before_s = id(s6)
s6 |= {2}
print(type(s6).__name__, id(s6) != before_s)


# --- reflected __ror__ on RHS used when __ior__ returns NotImplemented ---
class SRefl(set):
    def __ior__(self, other):
        return NotImplemented


class HasRor:
    def __ror__(self, other):
        return "ror!"


s7 = SRefl({1})
s7 |= HasRor()
print(s7)


# --- incompatible RHS: TypeError with the |= symbol (override + no override) ---
class SBadOr(set):
    def __ior__(self, other):
        return NotImplemented


try:
    sb = SBadOr({1})
    sb |= [1, 2]
except TypeError as e:
    print(e)


class SBadPlain(set):
    pass


try:
    sp = SBadPlain({1})
    sp |= [1, 2]
except TypeError as e:
    print(e)
