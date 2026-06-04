# Issue #2076: a `__slots__` instance (no '__dict__' slot, no unslotted base)
# exposes no `__dict__`.
class S:
    __slots__ = ("x",)


s = S()
s.x = 1
print(hasattr(s, "__dict__"))    # False
print(getattr(s, "__dict__", "NODICT"))  # NODICT
try:
    s.__dict__
except AttributeError as e:
    print(e)                     # 'S' object has no attribute '__dict__'
try:
    vars(s)
except TypeError as e:
    print(e)                     # vars() argument must have __dict__ attribute
try:
    s.y = 2
except AttributeError as e:
    print(e)                     # 'S' object has no attribute 'y'

# Exception 1: '__dict__' in __slots__ keeps a working __dict__, and the other
# slots are still member descriptors hidden from that __dict__.
class D:
    __slots__ = ("q", "__dict__")


d = D()
print(hasattr(d, "__dict__"))    # True
print(type(D.q).__name__)        # member_descriptor
d.q = 1
d.free = 2
print(d.q, d.free)               # 1 2
print(d.__dict__)                # {'free': 2}  (slot 'q' not in __dict__)
print(vars(d))                   # {'free': 2}

# Exception 2: a non-slotted base reintroduces __dict__ for the subclass.
class Base:
    pass


class Child(Base):
    __slots__ = ("z",)


c = Child()
print(hasattr(c, "__dict__"))    # True
c.anything = 9
c.z = 3
print(c.anything, c.z)           # 9 3

# Inheritance: all-slots chain still suppresses __dict__.
class P:
    __slots__ = ("p",)


class Q(P):
    __slots__ = ("r",)


q = Q()
q.p = 1
q.r = 2
print(hasattr(q, "__dict__"))    # False
print(q.p, q.r)                  # 1 2
