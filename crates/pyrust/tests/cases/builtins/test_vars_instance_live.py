# `vars(instance)` returns a live mutable proxy for the instance's `__dict__`
# (#1027).
#
# CPython 3.12 returns the *same* dict object on every `vars(obj)` call for
# the same instance.  Mutations through the returned dict propagate to the
# instance (and vice versa).  Previously pyrust returned a snapshot copy, so
# writes were silently discarded.

# ── Live view: attribute set on instance is visible via vars() ──────────
class C:
    pass

c = C()
c.x = 1
d = vars(c)
c.y = 2
print('y' in d)          # True  (live view, not snapshot)

# ── Mutation through vars() propagates to the instance ───────────────────
d['z'] = 3
print(c.z)               # 3

# ── Identity: vars(obj) is vars(obj) ──────────────────────────────────────
print(vars(c) is vars(c))    # True

# ── Identity: obj.__dict__ is vars(obj) ───────────────────────────────────
print(c.__dict__ is vars(c))  # True

# ── Separate instances have distinct dicts ────────────────────────────────
c2 = C()
print(vars(c) is vars(c2))   # False

# ── Deletion through vars() removes the attribute ─────────────────────────
del vars(c)['x']
try:
    _ = c.x
    print("x still exists")  # should not reach
except AttributeError:
    print("x deleted OK")    # x deleted OK

# ── Empty instance ────────────────────────────────────────────────────────
class D:
    pass

print(vars(D()))             # {} — temporary instance, must not crash

# ── Exception instance: hidden C-level slots not in vars() ────────────────
e = Exception("test")
ev = vars(e)
print('args' not in ev)      # True
print('__cause__' not in ev) # True

# ── Identity holds for exception instances too ────────────────────────────
print(vars(e) is vars(e))    # True

# ── vars(instance) == __dict__ in content ─────────────────────────────────
class G:
    def __init__(self):
        self.p = 1
        self.q = 2

g = G()
print(vars(g) == g.__dict__)  # True
