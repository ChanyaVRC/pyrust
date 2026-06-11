# Parity fixture for issue #2339 (LoadCell / StoreCell opcode stage of #452).
#
# Function-scope cell variables (names captured by a nested function) and
# `nonlocal` declarations now read/write through dedicated `LoadCell` /
# `StoreCell` opcodes instead of the name-keyed `LoadGlobal` / `StoreGlobal`
# path.  The backing store is still the env, so every observable behaviour must
# be unchanged.  These cases exercise the full landmine matrix the issue lists:
# multi-level nonlocal, sibling cell sharing, del-of-cell, walrus promotion,
# generators capturing a cell over a suspension, class-body scoping (both
# directions), deep nesting, and `__closure__` / `co_freevars` introspection
# (which is built by scanning the cell-read opcodes).

# --- single-cell free read (LoadCell on the free var `n`) ---
def make_adder(n):
    def add(x):
        return x + n
    return add

print(make_adder(10)(5))    # 15

# --- nonlocal aug-assign hot path (LoadCell + StoreCell on `c`) ---
def counter():
    c = 0
    def inc():
        nonlocal c
        c += 1
        return c
    return inc

f = counter()
print(f(), f(), f())        # 1 2 3

# --- multi-level nonlocal across three function scopes ---
def a():
    x = 1
    def b():
        nonlocal x
        def c():
            nonlocal x
            x += 10
        c()
        x += 1
    b()
    return x

print(a())                  # 12

# --- two sibling closures sharing one parent cell ---
def make_pair():
    n = 0
    def inc():
        nonlocal n
        n += 1
        return n
    def get():
        return n
    return inc, get

inc, get = make_pair()
inc(); inc()
print(get())                # 2

# --- del of a cell var then read -> NameError ---
def deltest():
    y = 5
    def inner():
        return y
    del y
    try:
        inner()
    except NameError:
        return "NameError"
    return "no error"

print(deltest())            # NameError

# --- deep nesting: 5 function levels, innermost mutates the outermost cell ---
def l1():
    v = 0
    def l2():
        def l3():
            def l4():
                def l5():
                    nonlocal v
                    v += 1
                    return v
                return l5()
            return l4()
        return l3()
    return l2()

print(l1())                 # 1

# --- walrus promotion: `y` escapes the comprehension into the enclosing scope ---
def wal():
    data = [1, 2, 3, 4]
    res = [y := n * 2 for n in data]
    return y, res

print(wal())                # (8, [2, 4, 6, 8])

# --- suspended generator capturing a cell, resumed after an external mutation ---
def gentest():
    s = 0
    def g():
        nonlocal s
        for i in range(3):
            s += i
            yield s
    it = g()
    out = [next(it)]
    s += 100
    out.append(next(it))
    out.append(next(it))
    return out

print(gentest())            # [0, 101, 103]

# --- class body reads an enclosing function cell (free var into class scope) ---
def classtest():
    base = 100
    class C:
        val = base + 1
    return C.val

print(classtest())          # 101

# --- mixed: cell + module global + builtin resolved in the same function ---
G = "module_global"

def mixed():
    c = 1
    def inner():
        nonlocal c
        c += 1
        return (c, G, len("abc"))
    return inner()

print(mixed())              # (2, 'module_global', 3)

# --- __closure__ / co_freevars built from the cell-read opcodes ---
def closuretest():
    cap = 42
    extra = 7
    def fn():
        return cap + extra
    return fn

fn = closuretest()
print([cell.cell_contents for cell in fn.__closure__])  # [42, 7]
print(fn.__code__.co_freevars)                          # ('cap', 'extra')
