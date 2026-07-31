# Issue #3024: a function frame's `locals()` is `co_varnames` + `co_cellvars` +
# `co_freevars`.  A cell variable (a local captured by a nested function) and a
# free variable (a name read from an enclosing function scope) both live outside
# the register file, so a registers-only walk drops them.
#
# Version-stable: only membership, values, and ordering are read — never the
# mapping's type or identity, and nothing is written through it, so PEP 667's
# FrameLocalsProxy (3.13+) does not change any output here.  The `co_cellvars` /
# `co_freevars` tail order (each group sorted by name, after the fastlocals) is
# the same on 3.11 / 3.12 / 3.13.
import sys

# --- the captured local is visible in the frame that owns it ---------------


def owner():
    x = 1

    def inner():
        return x

    print("owner:", sorted(locals()), locals().get("x"))


owner()

# --- and in the frame that captures it -------------------------------------


def capture():
    captured = 1
    untouched = 2

    def reader():
        own = 3
        print("reader:", sorted(locals()), locals().get("captured"))
        return captured, own

    reader()
    print("capture:", sorted(locals()))


capture()

# --- an unbound cell is omitted until it is bound, and again after `del` ----


def binding_lifecycle():
    print("before bind:", sorted(locals()))
    x = 1

    def inner():
        return x

    print("after bind:", sorted(locals()), locals().get("x"))
    del x
    print("after del:", sorted(locals()))
    x = 2
    print("after rebind:", sorted(locals()), locals().get("x"))


binding_lifecycle()


def unbound_free():
    def early():
        print("free before bind:", sorted(locals()))

    early()
    x = 5

    def late():
        print("free after bind:", sorted(locals()), locals().get("x"))
        return x

    late()


unbound_free()

# --- a `nonlocal` write is visible from both frames ------------------------


def nonlocal_write():
    x = 1

    def inner():
        nonlocal x
        x = 99
        print("nonlocal inner:", sorted(locals()), locals().get("x"))

    inner()
    print("nonlocal outer:", locals().get("x"))


nonlocal_write()


def nonlocal_and_free():
    zz = 1
    aa = 2

    def keep():  # makes both names cells of this frame
        return zz + aa

    def write_only():
        nonlocal zz
        zz = 3
        # `zz` is never read here, so no read instruction reveals it — it is
        # still a free variable, and sorts with the rest of that group.
        print("nonlocal write-only + free:", list(locals()))
        return aa

    write_only()

    def read_write():
        nonlocal zz
        local_v = 1
        zz = zz + 1
        print("nonlocal read-write + free:", list(locals()))
        return aa

    read_write()
    print("nonlocal owner:", list(locals()), keep())


nonlocal_and_free()

# --- ordering: fastlocals, then cells sorted, then frees sorted ------------


def order_cells():
    zeta = 1
    alpha = 2
    plain = 3

    def inner():
        return zeta + alpha

    print("order cells:", list(locals()), plain)


order_cells()


def order_outer():
    beta = 1
    alp = 2

    def mid():
        yy = 0
        xx = 1

        def deep():
            return xx + yy + alp + beta

        print("order cells+frees:", list(locals()))
        deep()

    mid()


order_outer()

# --- a cell parameter binds into the cell, not the register ----------------


def cell_param(p, q):
    def inner():
        return p

    print("cell param:", sorted(locals()), locals().get("p"), locals().get("q"))


cell_param(1, 2)

# --- a cell shadowing a module global, and an explicit `global` --------------
shadowed = "module"


def shadowing():
    shadowed = "cell"

    def inner():
        return shadowed

    print("shadowing:", sorted(locals()), locals().get("shadowed"))


shadowing()


def global_decl():
    shadowed = "enclosing"

    def writer():
        global shadowed
        shadowed = "written"
        print("global decl:", sorted(locals()))

    writer()

    def reader():
        print("free read:", sorted(locals()), locals().get("shadowed"))
        return shadowed

    reader()
    print("global decl outer:", locals().get("shadowed"))


global_decl()


# A `global` declaration that *reads* the name is the case that distinguishes a
# real filter from an accident: `writer` above only stores, which compiles to no
# read, so the name never reaches the candidate set at all.  Here the enclosing
# scope binds `probed` as a cell and the declaring body reads it, so the name is
# a live candidate that resolves in an enclosing function scope — it must still
# be filtered out, in a plain call and in a repeated (trampolined) one, and
# whether or not the body has any other reason to own a local env.
probed = "module probed"


def global_read_decl():
    probed = "enclosing"

    def keep():  # makes `probed` a cell of this frame
        return probed

    def declares_global():
        global probed
        return sorted(locals()), probed

    def declares_global_with_local():
        global probed
        own = 1
        return sorted(locals()), probed, own

    def free_reader():
        return sorted(locals()), probed

    print("global read decl:", declares_global())
    print("global read decl + local:", declares_global_with_local())
    print("free sibling:", free_reader())

    # Repeat past the point where the call trampoline takes over: it publishes
    # no env for the frame, so the declarations have to come from somewhere the
    # trampolined frame can still reach.
    last = None
    for _ in range(200):
        last = declares_global()
    print("global read decl (repeated):", last)
    keep()


global_read_decl()


def global_read_decl_gen():
    probed = "enclosing gen"

    def keep():
        return probed

    def gen():
        global probed
        yield sorted(locals()), probed

    print("global read decl gen:", next(gen()))
    keep()


global_read_decl_gen()
print("module global after:", shadowed)

# --- lambdas, methods, generators, coroutines ------------------------------


def lambda_frame():
    v = 5
    f = lambda: (sorted(locals()), locals().get("v"), v)
    print("lambda:", f())


lambda_frame()


def method_frame():
    tag = "T"

    class K:
        def m(self, a):
            b = a + 1
            print("method:", sorted(locals()), locals().get("tag"))
            return tag, b

    K().m(1)


method_frame()


def generator_frame():
    outer_seen = 10

    def gen(a):
        b = a + 1
        yield sorted(locals())
        c = b + 1

        def uses_c():
            return c

        yield sorted(locals()), locals().get("outer_seen")

    g = gen(1)
    print("gen 1:", next(g))
    print("gen 2:", next(g))
    print("gen owner:", sorted(locals()))
    for _ in g:
        pass


generator_frame()


def coroutine_frame():
    cap = 9

    async def co(x):
        y = x + 1
        print("coroutine:", sorted(locals()), locals().get("cap"))
        return cap, y

    coro = co(1)
    try:
        coro.send(None)
    except StopIteration:
        pass


coroutine_frame()

# --- a class body sees neither the enclosing cell nor a free variable ------


def class_body():
    cv = 7

    class C:
        a = 1
        print("class body:", [k for k in locals() if not k.startswith("__")])
        b = cv

    return C.b


print("class body value:", class_body())

# --- recursion through the self-call path keeps the free variable ----------


def recursive():
    cap = 100

    def rec(n):
        if n == 0:
            return sorted(locals()), locals().get("rec") is rec
        return rec(n - 1)

    print("recursive:", rec(5), cap)


recursive()

# --- `vars()` and an outer frame's `f_locals` see the same namespace -------


def outer_frame_read():
    x = 1

    def inner():
        return x

    def probe():
        outer = sys._getframe(1).f_locals
        print("outer f_locals:", sorted(outer), outer.get("x"))

    probe()
    print("vars():", sorted(vars()))


outer_frame_read()
