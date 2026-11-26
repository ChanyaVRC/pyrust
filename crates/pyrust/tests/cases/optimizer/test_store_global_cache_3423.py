# Parity fixture for the StoreGlobal inline cache and for global-declaring
# callees on the call trampoline (issue #3423).
#
# A repeated `global G; G = v` overwrites the cached root binding in place, and
# a LoadGlobal of a bound global reads that same slot, so no cache generation
# moves on a rebind. Every section therefore warms a cache first and then
# checks one way the cached target can stop being right: a reader in another
# function, a module-level rebind, deletion and re-creation, a name that
# shadows a builtin, `globals()` exposure, another module's namespace, and a
# nested defining scope.
import _store_global_cache_caller_3423 as caller
import _store_global_cache_owner_3423 as owner

# -- hot overwrite, then every reader sees it --------------------------------
print("== hot overwrite ==")
G = 0


def write_loop(n):
    global G
    for i in range(n):
        G = i


def read_g():
    return G


print(read_g())
write_loop(50)
print(G, read_g())
write_loop(7)
print(G, read_g())

# -- read-modify-write in one function ----------------------------------------
print("== read-modify-write ==")


def add_loop(n):
    global G
    for i in range(n):
        G += 1


G = 0
add_loop(100)
print(G, read_g())
add_loop(100)
print(G, read_g())

# -- a warm reader interleaved with a warm writer -----------------------------
print("== interleaved reader/writer ==")


def set_g(value):
    global G
    G = value


seen = []
for i in range(6):
    set_g(i * 10)
    seen.append(read_g())
print(seen)

# -- module-level rebinding between cached stores ------------------------------
print("== module rebinding ==")
G = 500
print(read_g())
set_g(501)
print(G, read_g())
G = G + 1
print(G, read_g())
add_loop(3)
print(G, read_g())
for G in range(3):
    pass
print(G, read_g())
set_g("text")
print(G, read_g())

# -- two globals and an unrelated global callee stay coherent ------------------
print("== two globals ==")
A = 0
B = 0


def helper(x):
    return x + 1


def two(n):
    global A, B
    for i in range(n):
        A = helper(A)
        B += len("ab")
    return A, B


print(two(20), A, B)
print(two(5), A, B)

# -- value kinds: the cache locates a slot, never a value ----------------------
print("== value kinds ==")
V = None


def set_v(value):
    global V
    V = value


def read_v():
    return V


for value in (1, 2.5, "a heap allocated string value", [1, 2], None, (1, "x"), {"k": 1}):
    set_v(value)
    print(type(read_v()).__name__, read_v() is value, V is value)

# -- delete, then rebind -------------------------------------------------------
print("== del then rebind ==")
D = 1


def set_d(value):
    global D
    D = value


def del_d():
    global D
    del D


def read_d():
    return D


def del_and_set_d(value):
    global D
    del D
    D = value


set_d(2)
set_d(3)
print(read_d())
del_d()
try:
    read_d()
except NameError as exc:
    print("NameError:", exc)
set_d(4)
print(D, read_d())
set_d(5)
print(D, read_d())
del D
try:
    read_d()
except NameError as exc:
    print("NameError:", exc)
set_d(6)
print(D, read_d())
for i in range(4):
    del_and_set_d(i)
    print(D, read_d(), end=" | ")
print()

# -- a name that exists only through `global` writes ---------------------------
print("== global-only name ==")


def define(value):
    global ONLY_HERE
    ONLY_HERE = value


def read_only_here():
    return ONLY_HERE


for i in range(5):
    define(i)
print(ONLY_HERE, read_only_here())
ONLY_HERE = "module"
print(read_only_here())
define("function")
print(ONLY_HERE, read_only_here())

# -- shadowing and unshadowing a builtin ---------------------------------------
print("== builtin shadow ==")


def call_len(x):
    return len(x)


def shadow_len(fn):
    global len
    len = fn


def unshadow_len():
    global len
    del len


print(call_len("abc"))
shadow_len(lambda x: 40)
print(call_len("abc"))
shadow_len(lambda x: 41)
print(call_len("abc"))
shadow_len(lambda x: 42)
print(call_len("abc"))
unshadow_len()
print(call_len("abc"))
shadow_len(lambda x: 43)
print(call_len("abc"))
unshadow_len()
print(call_len("abc"))

# -- container write-back of a global that is not declared `global` ------------
print("== container write-back ==")
TABLE = {}
ITEMS = [0, 0, 0]


def fill(n):
    for i in range(n):
        TABLE[i % 3] = i
        ITEMS[i % 3] += i


fill(10)
print(TABLE, ITEMS)
fill(4)
print(TABLE, ITEMS)

# -- a global written from a method (defining scope is the class body) ---------
print("== method ==")
TOTAL = 0


class Counter:
    TOTAL = "class attribute"

    def add(self, step):
        global TOTAL
        TOTAL += step
        return TOTAL

    def set(self, value):
        global TOTAL
        TOTAL = value


def read_total():
    return TOTAL


c = Counter()
print([c.add(i) for i in range(6)])
print(TOTAL, read_total(), Counter.TOTAL, c.TOTAL)
c.set(-1)
c.set(-2)
print(TOTAL, read_total(), Counter.TOTAL)
del TOTAL
c.set(9)
print(TOTAL, read_total(), c.add(1), Counter.TOTAL)


class Outer:
    class Inner:
        def set(self, value):
            global NESTED
            NESTED = value


inner = Outer.Inner()
for i in range(4):
    inner.set(i)
print(NESTED)

# -- a generator that rebinds a global ----------------------------------------
print("== generator ==")
STEP = 0


def stepper(n):
    global STEP
    for i in range(n):
        STEP = i
        yield STEP


print(list(stepper(5)), STEP)
gen = stepper(3)
print(next(gen), STEP)
STEP = 99
print(next(gen), STEP)

# -- trampolined callees that declare `global` ---------------------------------
print("== trampoline ==")
CALLS = 0
LAST = None


def tick(x):
    global LAST
    LAST = x


def count(x):
    global CALLS
    CALLS += 1
    return x


def run(n):
    for i in range(n):
        tick(count(i))


run(40)
print(CALLS, LAST)


def fact(n):
    global CALLS
    CALLS += 1
    if n <= 1:
        return 1
    return n * fact(n - 1)


CALLS = 0
print(fact(12), CALLS)


def stepped(x, step=10):
    global LAST
    LAST = x + step
    return LAST


def run_defaults(n):
    out = []
    for i in range(n):
        out.append(stepped(i))
        out.append(stepped(i, 1))
    return out


print(run_defaults(3), LAST)


class Machine:
    def press(self, value):
        global LAST
        LAST = value
        return self


def run_methods(n):
    machine = Machine()
    for i in range(n):
        machine.press(i).press(i + 100)


run_methods(5)
print(LAST)


def failing(x):
    global LAST
    LAST = x
    return 1 // x


def run_failing():
    total = 0
    for i in (3, 2, 1, 0, 5):
        total += failing(i)
    return total


try:
    run_failing()
except ZeroDivisionError as exc:
    print("ZeroDivisionError:", exc, LAST)
tick("after")
print(LAST)

# -- a recursion that only ends at the limit still counts its frames ------------
print("== recursion limit ==")
DEPTH = 0


def deep(n):
    global DEPTH
    DEPTH = n
    return deep(n + 1)


try:
    deep(1)
except RecursionError as exc:
    print("RecursionError:", exc, DEPTH > 100)
tick("recovered")
print(LAST)

# -- a trampolined global writer keeps its traceback frame ----------------------
print("== traceback ==")
import sys
import traceback


def boom(x):
    global LAST
    LAST = x
    raise ValueError("boom")


def calls_boom(x):
    boom(x)
    return x


try:
    calls_boom("raised")
except ValueError:
    frames = traceback.extract_tb(sys.exc_info()[2])
    print([frame.name for frame in frames], LAST)


def frame_names(x):
    global LAST
    LAST = x
    frame = sys._getframe()
    return frame.f_code.co_name, frame.f_back.f_code.co_name, sorted(frame.f_locals)


def calls_frame_names(x):
    return frame_names(x)


print(calls_frame_names("framed"), LAST)

# -- hot rebinding at module level of a name functions read ---------------------
print("== module-level store ==")
running = 0


def read_running():
    return running


checks = []
for i in range(8):
    running += i
    checks.append(read_running() == running)
print(running, read_running(), all(checks))
running = "reset"
print(read_running())

# -- two writers of one name, and a reader warmed between them ------------------
print("== two writers ==")
SHARED = 0


def first_writer(value):
    global SHARED
    SHARED = value


def second_writer(value):
    global SHARED
    SHARED = value * 2


def read_shared():
    return SHARED


out = []
for i in range(5):
    first_writer(i)
    out.append(read_shared())
    second_writer(i)
    out.append(read_shared())
print(out, SHARED)

# -- names whose cache-interest bits are covered by other names' bits ------------
# The interest sets are two-bit Bloom masks: `cover_20` shares one bit with
# `cover_0` and the other with `cover_10`, so once those two are rebound in
# place `cover_20` looks registered although it never was. A reader that cached
# its value before that must still observe every later rebind.
print("== covered interest mask ==")
cover_0 = 0
cover_10 = 0
cover_20 = 0


def set_cover_0(value):
    global cover_0
    cover_0 = value


def set_cover_10(value):
    global cover_10
    cover_10 = value


def set_cover_20(value):
    global cover_20
    cover_20 = value


def read_cover_20():
    return cover_20


set_cover_20("a")
print(read_cover_20(), read_cover_20())
for i in range(3):
    set_cover_0(i)
    set_cover_10(i)
print(read_cover_20())
set_cover_20("b")
print(read_cover_20(), cover_20)
set_cover_20("c")
print(read_cover_20(), cover_20)
for i in range(3):
    set_cover_0(i)
    set_cover_20(i)
    print(read_cover_20(), end=" ")
print(cover_0, cover_10, cover_20)

# -- deleting an earlier global shifts the slots behind it ----------------------
print("== shifted slot ==")
EARLY = "early"
BEHIND = 0


def set_behind(value):
    global BEHIND
    BEHIND = value


def read_behind():
    return BEHIND


set_behind(1)
set_behind(2)
print(read_behind())
del EARLY
set_behind(3)
print(BEHIND, read_behind())
set_behind(4)
print(BEHIND, read_behind())
EARLY = "again"
set_behind(5)
print(BEHIND, read_behind(), EARLY)

# -- writers defined in nested scopes -------------------------------------------
print("== nested definitions ==")
NEST = 0


def make_adder(step):
    def add():
        global NEST
        NEST += step
        return NEST

    return add


add_two = make_adder(2)
add_five = make_adder(5)
print([add_two(), add_five(), add_two(), add_five()], NEST)
print([(WALRUS := i * 2) for i in range(4)], WALRUS)
print([(WALRUS := i * 3) for i in range(4)], WALRUS)


class Body:
    global BODY_GLOBAL
    BODY_GLOBAL = 5
    for _i in range(3):
        BODY_GLOBAL = BODY_GLOBAL + 1
    local_copy = BODY_GLOBAL


print(BODY_GLOBAL, Body.local_copy, hasattr(Body, "BODY_GLOBAL"))

# -- another module's globals: written there, never here -----------------------
print("== cross-module ==")
COUNT = "main"
LABEL = "main"
print(owner.ACC, owner.SQUARES)
print(caller.drive(5), owner.COUNT, owner.read(), caller.COUNT, COUNT)
print(caller.drive_direct(5), owner.COUNT, caller.COUNT, COUNT)
print(caller.drive_method(5), owner.COUNT, owner.Tally.COUNT, caller.COUNT, COUNT)
print(caller.own_bump(5), owner.COUNT, caller.COUNT, COUNT)
for i in range(4):
    owner.set_count(i)
print(owner.COUNT, owner.read())

# An attribute write from outside lands in the same binding the function reads
# and overwrites.
owner.COUNT = 70
print(owner.read(), owner.bump(1), owner.COUNT)
owner.relabel("one")
owner.relabel("two")
print(owner.LABEL, owner.read_label(), caller.LABEL, LABEL)

# Deleting the attribute and rebinding through the function.
del owner.COUNT
try:
    owner.read()
except NameError as exc:
    print("NameError:", exc)
owner.set_count(5)
owner.set_count(6)
print(owner.COUNT, owner.read(), hasattr(owner, "COUNT"))
owner.drop_and_rebind(7)
owner.drop_and_rebind(8)
print(owner.COUNT, owner.read())

# A name the module only gains through a `global` write.
print(hasattr(owner, "LATE"))
for i in range(3):
    owner.define_late(i)
print(owner.LATE, owner.read_late())

# The module dictionary observes every cached store, before and after it is
# exposed, and writes through it reach the functions.
owner.set_count(11)
namespace = vars(owner)
print(namespace["COUNT"], owner.read())
owner.set_count(12)
print(namespace["COUNT"], owner.COUNT, owner.read())
namespace["COUNT"] = 13
print(owner.read(), owner.bump(1), namespace["COUNT"], owner.COUNT)
print(caller.drive(3), namespace["COUNT"])

# -- globals() visibility --------------------------------------------------------
print("== globals() ==")
W = 0


def set_w(value):
    global W
    W = value


def read_w():
    return W


for i in range(5):
    set_w(i)
g = globals()
print(g["W"], g["G"], g["A"], g["B"], g["ONLY_HERE"])
set_w(10)
print(g["W"], W, read_w())
g["W"] = 20
print(W, read_w())
set_w(30)
print(g["W"], W, read_w())
del g["W"]
try:
    read_w()
except NameError as exc:
    print("NameError:", exc)
set_w(40)
print(g["W"], W, read_w())
set_w(50)
print(g["W"], W, read_w())
names = [name for name in g if name in ("G", "A", "B", "D", "W", "ONLY_HERE", "TOTAL", "NESTED")]
print(names)

# -- explicit exec namespaces keep their dictionary authoritative ---------------
print("== exec ==")
SOURCE = """
X = 0
def loop(n):
    global X
    for i in range(n):
        X = i
def bump():
    global X
    X += 1
    return X
def read():
    return X
"""
ns = {}
exec(SOURCE, ns)
ns["loop"](10)
print(ns["X"], ns["read"]())
ns["X"] = 100
print(ns["bump"](), ns["bump"](), ns["X"], ns["read"]())
del ns["X"]
ns["loop"](3)
print(ns["X"], ns["read"]())
other = {}
exec(SOURCE, other)
other["loop"](4)
print(other["X"], ns["X"])
