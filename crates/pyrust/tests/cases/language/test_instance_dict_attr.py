# `instance.__dict__` returns a dict of the instance's own attributes (#392).
#
# CPython exposes `__dict__` as the canonical mapping of an object's instance
# state.  It is the documented surface used by `pickle`, `copy.deepcopy`,
# dataclass-style introspection, and `**obj.__dict__` splats.
#
# pyrust returns a snapshot (clone) of the underlying attrs IndexMap — read
# access, key iteration, splatting and equality with `vars(obj)` all match
# CPython.  Mutating the returned dict does not propagate back to the
# instance; that "live dict" semantics is tracked as a follow-up.

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
