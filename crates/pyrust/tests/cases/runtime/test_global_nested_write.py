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

# --- Normal fastlocal write-back still works ---
# (a is never declared global, so it should be written back from regs)
a = 100
print(a)   # 100
