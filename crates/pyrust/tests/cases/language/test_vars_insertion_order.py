# `vars(obj)` and instance / class attrs preserve insertion order (#393).
#
# CPython 3.7+ guarantees every dict — including `__dict__` of class
# instances and class objects — iterates in insertion order.  pyrust
# previously stored attrs in `HashMap`, which produced non-deterministic
# orderings.  Switching the underlying store to `IndexMap` (and emitting
# class-body slots in textual order) is what this test pins.

# ── Instance attrs assigned directly ────────────────────────────────────
class C:
    pass

c = C()
c.x = 1
c.y = 2
c.z = 3
print(list(vars(c).keys()))                 # ['x', 'y', 'z']

# ── Instance attrs assigned via setattr ─────────────────────────────────
class S:
    pass

s = S()
setattr(s, "alpha", 10)
setattr(s, "beta", 20)
setattr(s, "gamma", 30)
print(list(vars(s).keys()))                 # ['alpha', 'beta', 'gamma']

# ── Instance attrs from __init__ ────────────────────────────────────────
class V:
    def __init__(self):
        self.first = 1
        self.second = 2
        self.third = 3

v = V()
print(list(vars(v).keys()))                 # ['first', 'second', 'third']

# ── Class attrs from a `class C: a=1; b=2` block ────────────────────────
# CPython injects `__module__` (and friends) before user names; pyrust
# does not synthesise those yet.  Filter dunders so we only compare the
# names contributed by the class body — those must stay in textual
# order across both implementations.
class D:
    a = 1
    b = 2
    c = 3

print([k for k in vars(D).keys() if not k.startswith("__")])  # ['a', 'b', 'c']

# ── Deletion preserves order of remaining keys ──────────────────────────
class E:
    pass

e = E()
e.p = 1
e.q = 2
e.r = 3
e.s = 4
del e.q
print(list(vars(e).keys()))                 # ['p', 'r', 's']

# ── `**obj.__dict__` splat into another dict preserves order ────────────
class F:
    pass

f = F()
f.one = 1
f.two = 2
f.three = 3
merged = {**vars(f), "four": 4}
print(list(merged.keys()))                  # ['one', 'two', 'three', 'four']

# ── Class with mixed defs and assignments stays in textual order ────────
class G:
    a = 1
    def m(self):
        return 2
    b = 3
    def n(self):
        return 4

# `vars(G)` includes the four class-body bindings in textual order
# (other dunders that CPython adds — `__module__`, `__qualname__`,
# `__dict__`, `__weakref__`, `__doc__` — vary across implementations
# and are not in pyrust's class dict, so we only assert the first four
# names match the textual order of the class body.
keys = list(vars(G).keys())
print([k for k in keys if k in {"a", "m", "b", "n"}])  # ['a', 'm', 'b', 'n']
