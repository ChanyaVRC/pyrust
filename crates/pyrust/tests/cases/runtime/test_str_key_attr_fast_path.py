# Parity fixture for issue #506: StrKey zero-alloc probe at attr-access sites.
#
# Covers the three sites changed in this PR:
#   1. UserFunction dynamic attr get (env.rs get_attr, UserFunction arm)
#   2. BoundMethod / PartialFunction dynamic attr get (env.rs get_attr, BuiltinFunction arm)
#   3. LoadGlobal fallback via module_globals_dict (vm.rs Insn::LoadGlobal)
#
# The parity test just verifies correct behaviour; allocation savings are
# verified separately via hyperfine.

# ── UserFunction dynamic attrs ────────────────────────────────────────────────

def my_func():
    pass

# Assign and read back a dynamic attribute on a plain function.
my_func.custom = "hello"
print(my_func.custom)          # hello

my_func.answer = 42
print(my_func.answer)          # 42

# Reading a non-existent dynamic attr raises AttributeError.
try:
    _ = my_func.no_such_attr
except AttributeError as e:
    print(type(e).__name__)    # AttributeError

# ── globals() dict mutation visible via LoadGlobal ───────────────────────────
# Writing through globals() bypasses assign_name, so the LoadGlobal fallback
# path (which probes module_globals_dict via StrKey) must pick it up.

globals()["_injected"] = "from_globals"
print(_injected)               # from_globals

globals()["_injected"] = "updated"
print(_injected)               # updated

# ── GetAttr / SetAttr / DeleteAttr round-trip on PyInstance ──────────────────

class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y

p = Point(3, 4)
print(p.x)                     # 3
print(p.y)                     # 4

p.x = 10
print(p.x)                     # 10

del p.x
try:
    _ = p.x
except AttributeError as e:
    print(type(e).__name__)    # AttributeError

# ── Dunder attribute access (GetAttr on special names) ───────────────────────

class Addable:
    def __add__(self, other):
        return self.__class__.__name__ + "+" + str(other)

a = Addable()
print(a + 1)                   # Addable+1

# ── Multiple attribute names (different hash buckets) ─────────────────────────

class Multi:
    pass

m = Multi()
for attr_name in ("alpha", "beta", "gamma", "delta"):
    setattr(m, attr_name, attr_name.upper())

for attr_name in ("alpha", "beta", "gamma", "delta"):
    print(getattr(m, attr_name))   # ALPHA, BETA, GAMMA, DELTA

# ── Attribute with same name as a dict string key (no cross-collision) ────────

d = {"x": "dict_x"}
class Holder:
    x = "class_x"
h = Holder()
h.x = "instance_x"
print(h.x)                     # instance_x
print(d["x"])                  # dict_x
