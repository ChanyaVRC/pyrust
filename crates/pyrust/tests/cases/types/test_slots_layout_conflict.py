# MI layout-conflict: two bases with non-empty __slots__ cannot be combined
# (CPython best_base/solid_base rule). Issue #2109.


def check(fn):
    try:
        fn()
        print("ok")
    except TypeError as e:
        print("TypeError:", e)


# Two distinct non-empty __slots__ -> conflict.
def two_distinct():
    class B1:
        __slots__ = ('a',)

    class B2:
        __slots__ = ('b',)

    class D(B1, B2):
        pass


check(two_distinct)


# Same slot name still conflicts (two layouts).
def same_name():
    class B1:
        __slots__ = ('a',)

    class B2:
        __slots__ = ('a',)

    class D(B1, B2):
        pass


check(same_name)


# One non-empty + one empty __slots__ -> allowed.
def one_empty():
    class F1:
        __slots__ = ('a',)

    class F2:
        __slots__ = ()

    class G(F1, F2):
        pass


check(one_empty)


# Both empty -> allowed.
def both_empty():
    class F1:
        __slots__ = ()

    class F2:
        __slots__ = ()

    class G(F1, F2):
        pass


check(both_empty)


# Dict-layout base (no __slots__) + slotted base -> allowed.
def dict_plus_slotted():
    class P1:
        pass

    class P2:
        __slots__ = ('a',)

    class G(P1, P2):
        pass


check(dict_plus_slotted)


# Two dict-layout bases -> allowed.
def two_dict():
    class P1:
        pass

    class P2:
        pass

    class G(P1, P2):
        pass


check(two_dict)


# Subtype relationship: D2(D1) then MI(D2, D1) -> allowed (related solid bases).
def subtype_related():
    class D1:
        __slots__ = ('a',)

    class D2(D1):
        __slots__ = ('b',)

    class G(D2, D1):
        pass


check(subtype_related)


# Diamond on a single slotted ancestor, empty-slots middles -> allowed.
def diamond_shared_solid():
    class Base:
        __slots__ = ('a',)

    class L(Base):
        __slots__ = ()

    class R(Base):
        __slots__ = ()

    class G(L, R):
        pass


check(diamond_shared_solid)


# Distinct slotted lineages -> conflict.
def distinct_lineages():
    class Base:
        __slots__ = ('a',)

    class L(Base):
        pass

    class R:
        __slots__ = ('b',)

    class G(L, R):
        pass


check(distinct_lineages)


# C-level conflict (int + str) still raises (#1677, unchanged).
def c_level():
    class X(int, str):
        pass


check(c_level)


# __slots__ = ('__dict__',) adds no real ivar -> shares object's layout.
def dict_slot_sentinel():
    class A:
        __slots__ = ('__dict__',)

    class B:
        __slots__ = ('x',)

    class G(A, B):
        pass


check(dict_slot_sentinel)


# Single-inheritance slot chain still works.
class S1:
    __slots__ = ('a',)


class S2(S1):
    __slots__ = ('b',)


s = S2()
s.a = 1
s.b = 2
print(s.a, s.b)


# Same rule applies to the 3-arg type() constructor.
def type_ctor_conflict():
    class B1:
        __slots__ = ('a',)

    class B2:
        __slots__ = ('b',)

    type('D', (B1, B2), {})


check(type_ctor_conflict)


def type_ctor_allowed():
    class F1:
        __slots__ = ('a',)

    class F2:
        __slots__ = ()

    type('G', (F1, F2), {})


check(type_ctor_allowed)
