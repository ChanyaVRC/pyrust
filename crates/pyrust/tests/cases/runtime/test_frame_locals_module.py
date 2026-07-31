# Issue #2926: a module frame's `f_locals` IS its `f_globals` (the live module
# dict), and outer frames (`sys._getframe(n)` for n > 0) expose their real local
# namespace instead of an empty dict.
#
# Version-stable: everything asserted here holds on 3.11 / 3.12 / 3.13.  The
# per-version parts of frame introspection (PEP 667's FrameLocalsProxy for
# *function* frames in 3.13+) are deliberately not exercised — only membership
# and values are read for function frames, never the mapping's type or identity.
import sys

# --- module frame identity, innermost and outer ---------------------------
f0 = sys._getframe()
print("self  f_locals is f_globals:", f0.f_locals is f0.f_globals)
print("self  f_locals is globals():", f0.f_locals is globals())


def caller_frame():
    return sys._getframe(1)


f1 = caller_frame()
print("outer f_locals is f_globals:", f1.f_locals is f1.f_globals)
print("outer f_locals is globals():", f1.f_locals is globals())
print("stable identity across calls:", caller_frame().f_locals is caller_frame().f_locals)


def via_back():
    return sys._getframe().f_back.f_locals is globals()


print("f_back.f_locals is globals():", via_back())

# --- the module namespace is readable through an outer frame ---------------
module_level_name = "visible"


def read_outer(name):
    return sys._getframe(1).f_locals.get(name, "<absent>")


print("outer read:", read_outer("module_level_name"))

# --- mid-assignment: the target is bound only once the statement completes --
fresh_target = read_outer("fresh_target")
print("fresh target during rhs:", fresh_target)

rebound = "before"
rebound = read_outer("rebound")
print("rebound target during rhs:", rebound)

pair_a = "A"
pair_b = "B"
pair_a, pair_b = read_outer("pair_a"), read_outer("pair_b")
print("tuple targets during rhs:", pair_a, pair_b)

# The same read taken directly on the module's own frame, with no call in
# between, must agree with the outer-frame read above.
direct = "prior"
direct = sys._getframe().f_locals.get("direct", "<absent>")
print("own frame during rhs:", direct)
direct_fresh = sys._getframe().f_locals.get("direct_fresh", "<absent>")
print("own frame fresh target:", direct_fresh)

# --- writes and deletes through a module frame hit the module namespace ----
doomed = 1


def mutate_outer():
    ns = sys._getframe(1).f_locals
    ns["planted"] = "planted-value"
    del ns["doomed"]


mutate_outer()
print("planted global:", planted)
print("doomed removed:", "doomed" in globals())

# --- depth walks -----------------------------------------------------------
def depth_three():
    return sorted(k for k in sys._getframe(2).f_locals if k in ("one", "two"))


def depth_two():
    return depth_three()


def depth_one():
    one = 1
    two = 2
    return depth_two()


print("depth 2 into a function frame:", depth_one())


def reach_module():
    return sys._getframe(2).f_locals is globals()


def middle():
    return reach_module()


print("depth 2 reaches the module frame:", middle())

# --- an outer function frame reports its own locals ------------------------
def peek_caller():
    return sorted(sys._getframe(1).f_locals.items())


def has_locals():
    alpha = 1
    beta = "b"
    return peek_caller()


print("outer function frame locals:", has_locals())


# --- an outer class-body frame reports the partially-built namespace -------
def peek_class_body():
    return sorted(k for k in sys._getframe(1).f_locals if not k.startswith("__"))


class Body:
    first = 1
    second = 2
    seen = peek_class_body()


print("outer class body locals:", Body.seen)

# --- generator bodies reach the module frame -------------------------------
def gen():
    yield sys._getframe(1).f_locals is globals()


print("generator body sees module frame:", next(gen()))

# --- lambdas and comprehensions --------------------------------------------
print("lambda sees module frame:", (lambda: sys._getframe(1).f_locals is globals())())

# --- exec frames: locals default to globals, else the caller's mapping -----
exec_globals = {"__name__": "exec_mod"}
exec(
    "import sys\n"
    "def look():\n"
    "    f = sys._getframe(1)\n"
    "    return (f.f_locals is f.f_globals, f.f_locals.get('inside'))\n"
    "inside = 7\n"
    "result = look()\n",
    exec_globals,
)
print("exec(globals only):", exec_globals["result"])

split_globals = {}
split_locals = {}
exec(
    "import sys\nf = sys._getframe()\nresult = f.f_locals is f.f_globals\n",
    split_globals,
    split_locals,
)
print("exec(globals, locals):", split_locals["result"])

# --- traceback frames ------------------------------------------------------
try:
    raise ValueError("boom")
except ValueError as exc:
    tb_frame = exc.__traceback__.tb_frame
    print("tb_frame.f_locals is globals():", tb_frame.f_locals is globals())
    print("tb_frame.f_locals is f_globals:", tb_frame.f_locals is tb_frame.f_globals)
