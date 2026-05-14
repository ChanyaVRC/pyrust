# `vars(obj)` and instance / class attrs preserve insertion order (#393).
#
# CPython 3.7+ guarantees every dict — including `__dict__` of class
# instances and class objects — iterates in insertion order.  pyrust
# previously stored attrs in `HashMap`, which produced non-deterministic
# orderings.  Switching the underlying store to `IndexMap` (with the class
# dict driven by **runtime stores** via `Insn::RecordClassStore` — not by
# a source-order walk) is what this test pins.

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

# ── `**vars(obj)` splat into another dict preserves order ─────────────
# `obj.__dict__` is also available (#392) and produces the same dict;
# a dedicated fixture in `test_instance_dict_attr.py` exercises it.
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


# ── Conditional branch — only the executed branch contributes ───────────
# The compiler used to pre-allocate slots by walking the body textually, so
# names from a never-executed `if False:` branch were inserted into the
# class dict ahead of names from the branch that actually ran.  After the
# Copilot fix, insertion order follows runtime stores, so the dead branch
# leaves no trace.
class C1:
    if False:
        b = 1
    else:
        a = 1
    b = 2

print([k for k in vars(C1).keys() if not k.startswith("__")])  # ['a', 'b']

# ── Loop in class body — first store wins on order, last store wins on value ──
# The for-loop iteration variable `i` is also a class-body local — its
# first stored value (from iteration 0) anchors its position in the dict;
# subsequent iterations update the value but not the order.
class C2:
    for i in range(3):
        x = i
    y = 99

print([k for k in vars(C2).keys() if not k.startswith("__")])  # ['i', 'x', 'y']

# ── `del` removes the entry and preserves the order of remaining keys ──
class C3:
    a = 1
    b = 2
    c = 3
    del b

print([k for k in vars(C3).keys() if not k.startswith("__")])  # ['a', 'c']

# ── Re-storing a name does NOT bump it to the end ───────────────────────
# CPython IndexMap semantics: updating an existing key keeps its position;
# only the *first* store determines order.
class C5:
    a = 1
    b = 2
    a = 3   # update — `a` keeps its first position
    c = 4

print([k for k in vars(C5).keys() if not k.startswith("__")])  # ['a', 'b', 'c']
