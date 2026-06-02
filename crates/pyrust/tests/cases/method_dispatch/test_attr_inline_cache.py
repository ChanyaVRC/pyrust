# Issues #1912 / #1998: instance-attribute read/write inline caches.
# Each scenario runs the access in a loop so the inline cache fills, then
# mutates the class to confirm the cache invalidates and produces the correct
# post-mutation value (a stale cache would print the old value here).


# --- plain instance read/write fast path ---------------------------------
class A:
    def __init__(self):
        self.x = 0


a = A()
for _ in range(5):
    a.x += 1
print("read/write", a.x)


# --- data descriptor (property) shadows the instance dict, both directions -
class P:
    @property
    def v(self):
        return 100

    @v.setter
    def v(self, val):
        self._stored = val * 2


p = P()
for _ in range(5):
    p.v = 3
print("property", p.v, p._stored)


# --- runtime data-descriptor add must invalidate the read cache ----------
class B:
    def __init__(self):
        self.x = 7


b = B()
print("pre-desc", b.x, b.x, b.x)
B.x = property(lambda self: 999)
print("post-desc", b.x, b.x)


# --- __class__ reassignment uses the NEW class (#1957/#2102) --------------
class C1:
    def who(self):
        return "C1"


class C2:
    @property
    def attr(self):
        return "C2-prop"

    def who(self):
        return "C2"


o = C1()
o.attr = "inst"
print("c1", o.who(), o.attr, o.who(), o.attr)
o.__class__ = C2
print("c2", o.who(), o.attr)


# --- runtime method add then call ----------------------------------------
class D:
    def __init__(self):
        self.x = 1


d = D()
for _ in range(3):
    print("before-method", d.x)
D.bump = lambda self: self.x + 41
print("after-method", d.bump(), d.bump())


# --- base-class data descriptor added after caching (epoch invalidation) --
class Base:
    pass


class Sub(Base):
    def __init__(self):
        self.q = "inst"


s = Sub()
print("base-pre", s.q, s.q)


class Desc:
    def __get__(self, obj, typ):
        return "desc"

    def __set__(self, obj, val):
        obj.__dict__["_q"] = val


Base.q = Desc()
print("base-post-read", s.q)
s.q = "written"
print("base-post-write", s.q, s.__dict__.get("_q"))


# --- runtime __setattr__ override invalidates the write cache -------------
class W:
    def __init__(self):
        self.v = 0


w = W()
w.v = 1
print("setattr-pre", w.v)
W.__setattr__ = lambda self, n, val: object.__setattr__(self, n, val + 100)
w.v = 1
print("setattr-post", w.v)


# --- __getattr__ fallback for a missing instance attr --------------------
class G:
    def __init__(self):
        self.real = 5

    def __getattr__(self, n):
        return "fallback:" + n


g = G()
for _ in range(3):
    print("getattr", g.real, g.missing)


# --- __slots__ write enforcement still raises ----------------------------
class Sl:
    __slots__ = ("a",)

    def __init__(self):
        self.a = 1


sl = Sl()
for _ in range(3):
    sl.a += 1
print("slots", sl.a)
try:
    sl.b = 9
except AttributeError:
    print("slots-err", "AttributeError")


# --- megamorphic site (two classes at one call site) ---------------------
class M1:
    def __init__(self):
        self.x = 1


class M2:
    def __init__(self):
        self.x = 2


for obj in (M1(), M2(), M1(), M2(), M1()):
    print("mega", obj.x)


# --- delete instance attr, access then resolves to the class attr --------
class Del:
    cv = "classval"


de = Del()
de.cv = "instval"
print("del-pre", de.cv, de.cv)
del de.cv
print("del-post", de.cv)
