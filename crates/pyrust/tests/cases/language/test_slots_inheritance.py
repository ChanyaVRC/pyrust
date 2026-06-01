# Issue #1892: __slots__ allowed-name set is the UNION across the MRO, not
# just the leaf class's __slots__.  Mirrors CPython 3.12.


# Base + derived both slotted: both slots settable on the leaf instance.
class G:
    __slots__ = ('a',)


class H(G):
    __slots__ = ('b',)


h = H()
h.a = 1
h.b = 2
print(h.a, h.b)
try:
    h.c = 3
except AttributeError as e:
    print(e)


# 3-level chain: every class's slot is settable on the deepest instance.
class A:
    __slots__ = ('x',)


class B(A):
    __slots__ = ('y',)


class C(B):
    __slots__ = ('z',)


c = C()
c.x = 1
c.y = 2
c.z = 3
print(c.x, c.y, c.z)
try:
    c.w = 4
except AttributeError as e:
    print(e)


# A slotless leaf reintroduces __dict__ -> arbitrary attributes allowed.
class P:
    __slots__ = ('p',)


class Q(P):
    pass


q = Q()
q.p = 1
q.arbitrary = 2
print(q.p, q.arbitrary)


# A slotless base also reintroduces __dict__ for a slotted leaf.
class R:
    pass


class S(R):
    __slots__ = ('s',)


s = S()
s.s = 1
s.free = 2
print(s.s, s.free)


# Empty __slots__ in a subclass: base slot still settable, no new ones.
class E1:
    __slots__ = ('e',)


class E2(E1):
    __slots__ = ()


e = E2()
e.e = 7
print(e.e)
try:
    e.f = 8
except AttributeError as err:
    print(err)


# String __slots__ (single slot).
class Str:
    __slots__ = 'single'


st = Str()
st.single = 5
print(st.single)
try:
    st.other = 6
except AttributeError as err:
    print(err)


# __dict__ declared in a base's __slots__ frees a slotted leaf's instances.
class W1:
    __slots__ = ('__dict__',)


class W2(W1):
    __slots__ = ('q',)


w2 = W2()
w2.q = 1
w2.free = 2
print(w2.q, w2.free)
