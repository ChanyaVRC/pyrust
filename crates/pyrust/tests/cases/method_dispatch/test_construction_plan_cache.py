# Construction-plan cache invalidation (issue #2330).
#
# instantiate_normal_instance caches the MRO-resolved __new__/__init__/primitive
# base per class, keyed on the class mutation_version + global class_epoch.  This
# fixture exercises every way the cached plan must be invalidated so that the
# *next* construction observes the change (CPython picks changes up on the next
# call).  Output must be byte-identical to python3.12.

# --- 1. Monkeypatch __init__ after the plan is cached by a first construction.
class P:
    def __init__(self, x):
        self.x = x


print(P(1).x)  # caches the plan


def new_init(self, x):
    self.x = x * 100


P.__init__ = new_init
print(P(2).x)  # must use the patched __init__

# --- 2. Monkeypatch __new__ after caching.
class Q:
    def __init__(self, x):
        self.x = x


print(Q(5).x)


def custom_new(cls, x):
    obj = object.__new__(cls)
    obj.tag = "new"
    return obj


Q.__new__ = custom_new
q = Q(7)
print(q.x, q.tag)

# --- 3. Mutate a BASE class after the subclass plan is cached.  The subclass
#        cache must invalidate via the global epoch even though the subclass
#        itself was not touched.
class Base:
    def __init__(self, v):
        self.v = v


class Sub(Base):
    pass


print(Sub(10).v)  # caches Sub's plan (inherits Base.__init__)


def base_new_init(self, v):
    self.v = v + 1000


Base.__init__ = base_new_init
print(Sub(20).v)  # must observe the mutated base __init__

# --- 4. Define a subclass later; both classes construct correctly.
class R:
    def __init__(self):
        self.k = 1


print(R().k)


class R2(R):
    def __init__(self):
        self.k = 2


print(R2().k, R().k)

# --- 5. Add __new__ then remove it; deletion must also invalidate.
class S:
    def __init__(self):
        self.z = 0


def s_new(cls):
    o = object.__new__(cls)
    o.z = 99
    return o


S.__new__ = s_new
print(S().z)  # caches the s_new plan
del S.__new__
print(S().z)  # back to object.__new__ + S.__init__

# --- 6. Primitive subclass repeated construction (caches prim classification).
class MyList(list):
    pass


print(list(MyList([1, 2, 3])), len(MyList([4, 5])))


class MyDict(dict):
    pass


d = MyDict()
d["a"] = 1
print(d["a"], len(MyDict()))

# --- 7. Repeated construction with a stable class (steady-state cache hit path).
class Counter:
    def __init__(self):
        self.n = 0

    def bump(self):
        self.n += 1
        return self.n


total = 0
for _ in range(1000):
    c = Counter()
    c.bump()
    total += c.n
print(total)
