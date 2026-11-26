# Regression for issue #3378: `o.x = v` on `__slots__` classes is served by an
# inline cache.  Every case drives ONE store site repeatedly (so the cache
# fills) and then mutates the class / instance shape behind it.


def store_x(o, v):
    o.x = v


def warm(o, n=5):
    for i in range(n):
        store_x(o, i)
    return o.x


# --- basic slot write, overwrite, delete, re-set -----------------------------
class S:
    __slots__ = ("x", "y")


s = S()
print("basic", warm(s))
store_x(s, "again")
print("overwrite", s.x)
del s.x
try:
    s.x
except AttributeError as e:
    print("deleted", e)
store_x(s, 42)
print("reset after del", s.x)
try:
    s.zzz = 1
except AttributeError as e:
    print("unknown slot", e)


# --- old value is released on overwrite ---------------------------------------
class Tracked:
    log = []

    def __init__(self, n):
        self.n = n

    def __del__(self):
        Tracked.log.append(self.n)


t = S()
warm(t, 3)
store_x(t, Tracked(1))
store_x(t, Tracked(2))
store_x(t, Tracked(3))
print("released", Tracked.log)
store_x(t, 0)
print("released2", Tracked.log)


# --- inheritance: parent slots + child slots ---------------------------------
class Base:
    __slots__ = ("x",)


class Child(Base):
    __slots__ = ("z",)


c = Child()
print("child inherited", warm(c))
store_x(c, "cc")
print("child inherited 2", c.x)
b = Base()
store_x(b, "bb")
print("base", b.x, c.x)  # same call site, now polymorphic


def store_z(o, v):
    o.z = v


for i in range(5):
    store_z(c, i)
print("child own", c.z)


# Same visible slot name in parent and child: distinct physical cells.
class P2:
    __slots__ = ("x",)


class C2(P2):
    __slots__ = ("x",)


c2 = C2()
for i in range(5):
    store_x(c2, i)
P2.x.__set__(c2, "parent-cell")
print("shadowed slot", c2.x, P2.x.__get__(c2))
store_x(c2, "child-cell")
print("shadowed slot 2", c2.x, P2.x.__get__(c2))


# --- __dict__ / __weakref__ slots keep CPython behaviour ---------------------
class D:
    __slots__ = ("x", "__dict__", "__weakref__")


d = D()
print("dict slot", warm(d))
d.other = 7
print("dict slot other", d.other, sorted(d.__dict__))


def store_dict(o, v):
    o.__dict__ = v


for _ in range(3):
    try:
        store_dict(d, 123)
    except TypeError as e:
        print("dict bad", e)
store_dict(d, {"q": 1})
print("dict replaced", d.__dict__, hasattr(d, "other"))


# --- same site, unrelated classes (megamorphic) ------------------------------
class Plain:
    pass


class Other:
    __slots__ = ("x",)


objs = [S(), Plain(), Other(), Child(), S(), Plain()]
for rnd in range(3):
    for o in objs:
        store_x(o, rnd)
print("poly", [o.x for o in objs])


# --- dynamic class mutation invalidates the cache ----------------------------
class M:
    __slots__ = ("x",)


m = M()
print("M warm", warm(m))
calls = []


def custom_setattr(self, name, value):
    calls.append((name, value))
    object.__setattr__(self, name, value)


M.__setattr__ = custom_setattr
store_x(m, "via-setattr")
print("custom __setattr__", calls, m.x)
del M.__setattr__
store_x(m, "after-del-setattr")
print("removed __setattr__", calls, m.x)

m2 = M()
print("M2 warm", warm(m2))
M.x_orig = M.x
vals = []
M.x = property(lambda self: self.x_orig.__get__(self), lambda self, v: vals.append(v))
store_x(m2, "prop")
print("property replaces slot", vals)
M.x = M.x_orig
store_x(m2, "restored")
print("slot restored", m2.x, vals)
# Base-class mutation after the subclass site warmed up.
class B3:
    __slots__ = ("x",)


class C3(B3):
    __slots__ = ()


c3 = C3()
print("C3 warm", warm(c3))
B3.__setattr__ = custom_setattr
calls.clear()
store_x(c3, "inherited-setattr")
print("base __setattr__", calls)
del B3.__setattr__


class DataDesc:
    def __init__(self):
        self.seen = []

    def __get__(self, obj, owner=None):
        return self

    def __set__(self, obj, value):
        self.seen.append(value)


dd = DataDesc()
B3.x = dd
store_x(c3, "to-descriptor")
print("base data descriptor", dd.seen)


# --- __class__ reassignment between layout-compatible slotted classes --------
class K1:
    __slots__ = ("x",)


class K2:
    __slots__ = ("x",)


k = K1()
print("K1 warm", warm(k))
k.__class__ = K2
store_x(k, "retyped")
print("retyped", type(k).__name__, k.x)


# --- read-only / special member behaviour ------------------------------------
class RO:
    __slots__ = ("x",)

    @property
    def y(self):
        return 1


r = RO()
print("RO warm", warm(r))


def store_y(o, v):
    o.y = v


for _ in range(3):
    try:
        store_y(r, 1)
    except AttributeError as e:
        print("no setter", e)


# --- receiver check on foreign instances goes through the descriptor ---------
try:
    S.x.__set__(object(), 1)
except TypeError as e:
    print("foreign receiver", e)

# --- class-level __slots__ with default object __setattr__ via super ---------
class Sup:
    __slots__ = ("x",)

    def __setattr__(self, name, value):
        super().__setattr__(name, value)


su = Sup()
print("Sup", warm(su))
