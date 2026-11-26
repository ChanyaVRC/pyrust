# Regression for issue #3382: `p.x` on a plain `@property` is served by a
# `PropertyGet` inline cache.  Every case drives ONE read site repeatedly (so
# the cache fills) and then mutates the class / instance shape behind it.


def read_x(o):
    return o.x


def warm(o, n=6):
    r = None
    for _ in range(n):
        r = read_x(o)
    return r


# --- basic read, instance state changes visible --------------------------------
class P:
    def __init__(self, v):
        self._v = v

    @property
    def x(self):
        return self._v * 2


p = P(3)
print("basic", warm(p))
p._v = 10
print("state change", read_x(p))
q = P(7)
print("other instance", read_x(q))


# --- property wins over an instance __dict__ entry -----------------------------
p.__dict__["x"] = "shadow"
print("dict shadow ignored", read_x(p), p.__dict__["x"])


# --- replacing the property on the class invalidates --------------------------
class R:
    @property
    def x(self):
        return "old"


r = R()
print("replace before", warm(r))
R.x = property(lambda self: "new")
print("replace property", read_x(r))
R.x = 42
print("replace with data attr", read_x(r))
R.x = lambda self: "method"
print("replace with method", read_x(r)())
del R.x
try:
    read_x(r)
except AttributeError as e:
    print("deleted", e)
R.x = property(lambda self: "back")
print("restored", read_x(r))


# --- instance attribute reachable once the property is removed -----------------
class D:
    @property
    def x(self):
        return "prop"


d = D()
warm(d)
del D.x
d.x = "inst"
print("after delete uses instance dict", read_x(d))


# --- base-class mutation invalidates (epoch) ----------------------------------
class Base:
    @property
    def x(self):
        return "base"


class Child(Base):
    pass


c = Child()
print("inherited", warm(c))
Base.x = property(lambda self: "base2")
print("base replaced", read_x(c))
Child.x = property(lambda self: "child")
print("child override", read_x(c))
del Child.x
print("child override removed", read_x(c))


# --- subclass overriding a parent's property ----------------------------------
class A:
    @property
    def x(self):
        return "A"


class B(A):
    @property
    def x(self):
        return "B"


class C(A):
    x = "data"


class M(A):
    def x(self):
        return "method"


for obj in (A(), A(), A(), B(), C(), M()):
    v = read_x(obj)
    print("override", type(obj).__name__, v() if callable(v) else v)


# --- __class__ reassignment ----------------------------------------------------
class K1:
    @property
    def x(self):
        return 1


class K2:
    @property
    def x(self):
        return 2


k = K1()
warm(k)
k.__class__ = K2
print("class reassigned", read_x(k))


# --- adding __getattribute__ / __getattr__ after the cache filled -------------
class G:
    @property
    def x(self):
        return "prop"


g = G()
warm(g)
G.__getattribute__ = lambda self, name: "custom:" + name
print("added __getattribute__", read_x(g))
del G.__getattribute__
print("removed __getattribute__", read_x(g))


class H:
    @property
    def x(self):
        raise AttributeError("from fget")

    def __getattr__(self, name):
        return "fallback:" + name


h = H()
for _ in range(5):
    print("getattr fallback", read_x(h))


class H2:
    @property
    def x(self):
        return "ok"


h2 = H2()
warm(h2)
H2.__getattr__ = lambda self, name: "fb"
print("added __getattr__ still property", read_x(h2))


# --- write-only / getter-less property ----------------------------------------
class W:
    def _set(self, v):
        self._v = v

    x = property(fget=None, fset=_set)


w = W()
for _ in range(4):
    try:
        read_x(w)
    except AttributeError as e:
        print("write-only", e)
w.x = 5
print("write-only stored", w._v)


# --- property setter / deleter still work through the cached read -------------
class S:
    def __init__(self):
        self._x = 0

    @property
    def x(self):
        return self._x

    @x.setter
    def x(self, v):
        self._x = v

    @x.deleter
    def x(self):
        self._x = -1


s = S()
warm(s)
s.x = 9
print("setter", read_x(s))
del s.x
print("deleter", read_x(s))


# --- exceptions from fget propagate, cache stays valid ------------------------
class E:
    def __init__(self):
        self.fail = True

    @property
    def x(self):
        if self.fail:
            raise ValueError("boom")
        return "fine"


e = E()
e.fail = False
warm(e)
e.fail = True
for _ in range(3):
    try:
        read_x(e)
    except ValueError as ex:
        print("raised", ex)
e.fail = False
print("recovered", read_x(e))


# --- traceback shape includes the fget frame ---------------------------------
import traceback


def tb_names():
    try:
        read_x(e_fail)
    except ValueError as ex:
        return [f.name for f in traceback.extract_tb(ex.__traceback__)]


e_fail = E()
e_fail.fail = False
warm(e_fail)
e_fail.fail = True
print("traceback", tb_names())


# --- polymorphic site goes megamorphic but stays correct ----------------------
class Pa:
    @property
    def x(self):
        return "pa"


class Pb:
    @property
    def x(self):
        return "pb"


for obj in (Pa(), Pa(), Pb(), Pa(), Pb(), Pb()):
    print("poly", read_x(obj))


# --- getter that recursively reads another property on the same site ----------
class Rec:
    def __init__(self, n):
        self.n = n

    @property
    def x(self):
        if self.n == 0:
            return 0
        return Rec(self.n - 1).x + 1


print("recursive", warm(Rec(5)))


# --- getter that mutates the class while cached -------------------------------
class Mut:
    @property
    def x(self):
        Mut.x = property(lambda self: "mutated")
        return "first"


m = Mut()
print("self-mutating", read_x(m), read_x(m), read_x(m))


# --- non-function getters stay on the generic path ---------------------------
class NF:
    x = property(len)


try:
    read_x(NF())
except TypeError as ex:
    print("builtin fget", type(ex).__name__)


class Callable_:
    def __call__(self, obj):
        return "callable-fget"


class CF:
    x = property(Callable_())


cf = CF()
for _ in range(4):
    print("callable fget", read_x(cf))


# --- functools.cached_property is not served by the property cache -----------
import functools


class CP:
    calls = 0

    @functools.cached_property
    def x(self):
        CP.calls += 1
        return "cp"


cp = CP()
for _ in range(4):
    read_x(cp)
print("cached_property calls", CP.calls)


# --- property with a base-class property overridden by an instance setter ----
class Dflt:
    @property
    def x(self):
        return getattr(self, "_x", "default")


dd = Dflt()
print("default", warm(dd))
dd._x = "set"
print("set", read_x(dd))
