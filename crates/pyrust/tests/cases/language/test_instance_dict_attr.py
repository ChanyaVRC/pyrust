# `instance.__dict__` returns a live mutable proxy for the instance's attrs
# (#1271 / #1272).
#
# CPython exposes `__dict__` as the canonical mapping of an object's instance
# state.  It is the documented surface used by `pickle`, `copy.deepcopy`,
# dataclass-style introspection, and `**obj.__dict__` splats.
#
# pyrust returns a live proxy backed by the actual attrs IndexMap — writes
# through the proxy propagate immediately to the instance, enabling data
# descriptor `__set__` implementations that store via `obj.__dict__['key'] = v`.

# ── Basic: instance with attrs ──────────────────────────────────────────
class C:
    pass

c = C()
c.x = 5
print(c.__dict__)                        # {'x': 5}

# ── Empty: instance with no attrs ───────────────────────────────────────
class D:
    pass

print(D().__dict__)                      # {}

# ── Class attrs are NOT included ────────────────────────────────────────
class E:
    cls_attr = 5

e = E()
print(e.__dict__)                        # {}

e.inst_attr = 7
print(e.__dict__)                        # {'inst_attr': 7}

# ── Insertion order matches assignment order ───────────────────────────
class F:
    pass

f = F()
f.b = 2
f.a = 1
f.c = 3
print(list(f.__dict__.keys()))           # ['b', 'a', 'c']

# ── __init__-populated attrs land in __dict__ in textual order ──────────
class G:
    def __init__(self):
        self.first = 1
        self.second = 2
        self.third = 3

g = G()
print(list(g.__dict__.keys()))           # ['first', 'second', 'third']

# ── Agreement with vars(obj) ────────────────────────────────────────────
class H:
    def __init__(self):
        self.p = 1
        self.q = 2

h = H()
print(h.__dict__ == vars(h))             # True

# ── `**obj.__dict__` splats into another dict in order ─────────────────
class I:
    pass

i = I()
i.one = 1
i.two = 2
merged = {**i.__dict__, "three": 3}
print(list(merged.keys()))               # ['one', 'two', 'three']

# ── Inheritance: subclass instance __dict__ holds only own attrs ───────
# CPython exposes only the instance's *own* attrs in __dict__ —
# attributes set on the parent class or on unrelated instances are not
# inherited into a subclass instance's __dict__.
class Base:
    pass

class Sub(Base):
    pass

sub = Sub()
sub.own = 7
print(sub.__dict__)                      # {'own': 7}

# Even if the base instance has its own attrs, those don't leak into a
# sibling subclass instance's __dict__ — they live on different objects.
b = Base()
b.base_only = 1
print(sub.__dict__)                      # still {'own': 7}

# ── `__dict__` itself is not a key in __dict__ ─────────────────────────
class J:
    pass

j = J()
j.x = 1
print("__dict__" in j.__dict__)          # False
