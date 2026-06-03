# Instance attribute storage behaviours (#2012).
#
# After switching `PyInstance` to interned-key compact storage, all observable
# attribute semantics must remain byte-identical to CPython 3.12: insertion
# order in __dict__, dynamic add/del, replacement, __slots__, inheritance, and
# the vars()/dir()/hasattr() reflection helpers.


class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y


# Basic get + __dict__ insertion order (CPython: insertion-ordered).
p = Point(1, 2)
print(p.x, p.y)
print(p.__dict__)
print(list(p.__dict__))

# Attribute added after construction appends at the end (order preserved).
p.z = 3
print(list(p.__dict__))
print(p.z)

# Overwrite keeps position, updates value.
p.x = 99
print(list(p.__dict__))
print(p.x)

# Deletion (del + delattr) preserves order of survivors.
del p.y
print(list(p.__dict__))
p.y = 7
print(list(p.__dict__))
delattr(p, "z")
print(list(p.__dict__))

# hasattr / getattr / setattr / vars.
print(hasattr(p, "x"), hasattr(p, "missing"))
print(getattr(p, "x"), getattr(p, "missing", "default"))
setattr(p, "w", 42)
print(vars(p))
print(vars(p) is vars(p))

# __dict__ mutation propagates back to the instance.
p.__dict__["k"] = 100
print(p.k)
print("k" in p.__dict__)
p.__dict__.update({"m": 200, "n": 300})
print(p.m, p.n)

# __dict__ replacement (assigning a whole new dict).
q = Point(10, 20)
q.__dict__ = {"a": 1, "b": 2, "c": 3}
print(list(q.__dict__))
print(q.a, q.b, q.c)
print(hasattr(q, "x"))

# Independence: two instances do not share storage.
a = Point(1, 2)
b = Point(3, 4)
a.x = 1000
print(a.x, b.x)
print(a.__dict__, b.__dict__)


# Inheritance: instance attrs vs class attrs.
class Base:
    cls_attr = "base-class-attr"

    def __init__(self):
        self.inst_attr = "base-inst"


class Derived(Base):
    def __init__(self):
        super().__init__()
        self.extra = "derived"


d = Derived()
print(d.inst_attr, d.extra, d.cls_attr)
print(list(d.__dict__))
print("cls_attr" in d.__dict__)  # class attr is NOT in instance __dict__
print(sorted(n for n in dir(d) if not n.startswith("_")))


# __slots__: slotted instances reject undeclared names and have no __dict__.
class Slotted:
    __slots__ = ("a", "b")

    def __init__(self, a, b):
        self.a = a
        self.b = b


s = Slotted(1, 2)
print(s.a, s.b)
s.a = 11
print(s.a)
try:
    s.c = 3
except AttributeError as e:
    print("AttributeError:", e)


# Many attributes (exercise the linear-storage growth path).
class Wide:
    def __init__(self):
        for i in range(20):
            setattr(self, "attr%d" % i, i)


w = Wide()
print(len(w.__dict__))
print(w.attr0, w.attr19)
print(list(w.__dict__) == ["attr%d" % i for i in range(20)])


# Empty instance.
class Empty:
    pass


e = Empty()
print(e.__dict__)
print(vars(e))
e.first = 1
print(e.__dict__)
