# Issue #3365: a data descriptor that defines `__set__` and/or `__delete__`
# but NOT `__get__` must not shadow the instance `__dict__` on reads.
#
# CPython's generic getattr only lets a class attribute win over the instance
# dict when its type has a `tp_descr_get` slot.  Without `__get__` the lookup
# order is: instance `__dict__` entry, else the descriptor object itself.


class NoGet:
    def __set__(self, o, v):
        o.__dict__["n"] = v


class DelOnly:
    def __delete__(self, o):
        o.__dict__.pop("d", None)


class Both:
    def __set__(self, o, v):
        o.__dict__["both"] = v

    def __delete__(self, o):
        o.__dict__.pop("both", None)


class B:
    n = NoGet()
    d = DelOnly()
    both = Both()


# --- the reported reproducer ---------------------------------------------
b = B()
print(type(b.n).__name__, type(b.d).__name__)
b.__dict__["n"] = 3
b.__dict__["d"] = 9
print(b.n, b.d)
b.n = 7
print(b.n)
del b.d
print(type(b.d).__name__)

# --- descriptor returned itself when there is no instance entry ----------
c = B()
print(c.n is B.__dict__["n"], c.d is B.__dict__["d"], c.both is B.__dict__["both"])
print(B.n is B.__dict__["n"], B.d is B.__dict__["d"])

# --- __set__ / __delete__ still run on writes and deletes ----------------
c.both = 11
print(c.both, c.__dict__)
del c.both
print(type(c.both).__name__, "both" in c.__dict__)

# --- hasattr / getattr / vars / object.__getattribute__ ------------------
e = B()
print(hasattr(e, "n"), getattr(e, "n", "missing") is B.__dict__["n"])
e.__dict__["n"] = "inst"
print(getattr(e, "n"), object.__getattribute__(e, "n"))
print(vars(e))


# --- repeated reads in one function (inline caches) ----------------------
def read_loop(obj, count):
    out = []
    for i in range(count):
        out.append(obj.n if isinstance(obj.n, str) else type(obj.n).__name__)
        if i == 2:
            obj.__dict__["n"] = "late"
        if i == 4:
            del obj.__dict__["n"]
    return out


print(read_loop(B(), 7))


# --- a data descriptor WITH __get__ still beats the instance dict --------
class WithGet:
    def __get__(self, o, t=None):
        return "descr-get" if o is not None else self

    def __set__(self, o, v):
        o.__dict__["g"] = v


class W:
    g = WithGet()


w = W()
w.__dict__["g"] = "shadow"
print(w.g)
w.g = "set"
print(w.g, w.__dict__["g"])
print(W.g is W.__dict__["g"])


# --- a non-data descriptor (only __get__): instance dict wins ------------
class OnlyGet:
    def __get__(self, o, t=None):
        return "only-get"


class N:
    x = OnlyGet()


n = N()
print(n.x)
n.__dict__["x"] = "inst"
print(n.x)


# --- adding / removing __get__ at runtime flips the precedence -----------
class Late:
    def __set__(self, o, v):
        pass


class L:
    f = Late()


inst = L()
inst.__dict__["f"] = "dict-value"
print(inst.f)
Late.__get__ = lambda self, o, t=None: "patched-get"
print(inst.f)
del Late.__get__
print(inst.f)


# --- inheritance: a subclass supplying __get__ ---------------------------
class Sub(NoGet):
    def __get__(self, o, t=None):
        return "sub-get"


class HasSub:
    n = Sub()


h = HasSub()
h.__dict__["n"] = "dict"
print(h.n)


# --- class attribute on a base class, instance dict on the subclass ------
class Derived(B):
    pass


dv = Derived()
print(type(dv.n).__name__)
dv.__dict__["n"] = "derived-dict"
print(dv.n)


# --- __getattr__ is only consulted when nothing is found -----------------
class WithGetattr:
    n = NoGet()

    def __getattr__(self, name):
        return "getattr:" + name


g = WithGetattr()
print(type(g.n).__name__)
g.__dict__["n"] = "d"
print(g.n)
print(g.missing)


# --- __slots__ -----------------------------------------------------------
class Slotted:
    __slots__ = ("a",)
    n = NoGet()


s = Slotted()
print(s.n is Slotted.__dict__["n"])
s.a = 5
print(s.a)
try:
    s.n = 1
except AttributeError as exc:
    # NoGet.__set__ writes to o.__dict__, which a slotted object lacks.
    print("AttributeError", "__dict__" in str(exc))


class SlottedDict:
    __slots__ = ("a", "__dict__")
    n = NoGet()


sd = SlottedDict()
sd.__dict__["n"] = "slot-dict"
print(sd.n)
sd.n = "via-set"
print(sd.n)


# --- super() ---------------------------------------------------------------
class Base:
    n = NoGet()


class Child(Base):
    n = "child-attr"

    def via_super(self):
        return super().n


ch = Child()
ch.__dict__["n"] = "inst"
print(ch.n, ch.via_super() is Base.__dict__["n"])


class Child2(Base):
    def via_super(self):
        return super().n


c2 = Child2()
c2.__dict__["n"] = "inst2"
# super() never consults the instance dict; it returns the raw descriptor.
print(c2.n, type(c2.via_super()).__name__)
print(type(super(Child2, c2).n).__name__, type(super(Child2, Child2).n).__name__)


# --- metaclass data descriptor without __get__ --------------------------
class MetaNoGet:
    def __set__(self, o, v):
        pass


class Meta(type):
    tag = MetaNoGet()


class WithMeta(metaclass=Meta):
    pass


print(type(WithMeta.tag).__name__)


class WithMetaOwn(metaclass=Meta):
    tag = "class-own"


print(WithMetaOwn.tag)


# --- an instance of a class whose descriptor is a callable object --------
class CallableDesc:
    def __call__(self, *a):
        return "called"

    def __set__(self, o, v):
        pass


class UsesCallable:
    m = CallableDesc()


u = UsesCallable()
print(type(u.m).__name__)
u.__dict__["m"] = lambda: "from-dict"
print(u.m())

# --- plain class attribute for comparison: no binding for a descriptor ---
print(type(B().n).__name__, B().both.__class__.__name__)
