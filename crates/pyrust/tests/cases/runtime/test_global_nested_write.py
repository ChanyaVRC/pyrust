# Exercises `global` declarations inside nested functions.
# The fastlocal write-back loop in program.rs must not clobber a value
# that StoreGlobal wrote to the module env during the script run (#520).

# --- Doubly-nested: global write from inner() must survive ---
x = 10

def outer():
    def inner():
        global x
        x = 99
    inner()

outer()
print(x)   # 99

# --- Singly-nested: direct global write ---
y = 1
def f():
    global y
    y = 2
f()
print(y)   # 2

# --- Multiple global increments accumulate correctly ---
z = 0
def inc():
    global z
    z += 1
inc()
inc()
inc()
print(z)   # 3

# --- Doubly-nested global increment: LoadGlobal must find the value in env ---
# (z is initialized at module scope; the doubly-nested inc() must both read and
#  write z via env.values because z is a cell var — not a fastlocal — after the
#  compiler promotes it due to the deep global declaration, #520)
w = 0
def outer_inc():
    def inc():
        global w
        w += 1
    inc()
    inc()
    inc()
outer_inc()
print(w)   # 3

# --- Normal fastlocal write-back still works ---
# (a is never declared global, so it should be written back from regs)
a = 100
print(a)   # 100
