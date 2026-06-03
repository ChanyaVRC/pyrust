# Parity fixture for #2123 — arguments are bound directly into the callee's
# new frame register file (and env, for cell-var params) instead of through an
# intermediate `Vec<Option<Value>>`.  A direct-write binding path can mis-handle
# the "already bound" tracking, falsy defaults, or the reg/cell split, so this
# exercises those edges and confirms byte-for-byte CPython parity.


# --- Falsy defaults must still land in their register, not be skipped ---
def falsy(a, b=0, c=False, d="", e=None, f=0.0):
    return (a, b, c, d, e, f)


print(falsy(1))
print(falsy(1, 9))
print(falsy(1, c=True, e="x"))
print(falsy(1, 2, 3, 4, 5, 6))


# --- Keyword routed to a register out of positional order ---
def order(a, b, c, d):
    return (a, b, c, d)


print(order(1, 2, d=4, c=3))
print(order(d=4, c=3, b=2, a=1))
print(order(1, d=4, b=2, c=3))


# --- bound_prefix (bound method) interleaved with defaults + keywords ---
class Acc:
    def add(self, x, y=10, *, z=100):
        return (x, y, z)


a = Acc()
print(a.add(1))
print(a.add(1, 2))
print(a.add(1, z=3))
print(a.add(x=1, z=3))


# --- Cell-var param and register param in the same frame ---
def mix_cell(captured, plain):
    def read():
        return captured

    return (read(), plain)


print(mix_cell(7, 8))
print(mix_cell(plain=8, captured=7))


# --- Cell-var param with a default that fires ---
def cell_default(x, step=5):
    def inc():
        nonlocal step
        step += 1
        return step

    return (x, inc(), inc())


print(cell_default(1))
print(cell_default(1, 100))


# --- Error paths: messages must match CPython exactly ---
def f(a, b, c):
    return a + b + c


def g(a, b, /, c):
    return a + b + c


def k(a, *, b):
    return a + b


errors = [
    lambda: f(1, 2),  # missing 1 positional
    lambda: f(1),  # missing 2 positional
    lambda: f(1, 2, 3, 4),  # too many positional
    lambda: f(1, 2, a=9),  # multiple values for 'a'
    lambda: f(1, 2, 3, z=9),  # unexpected keyword
    lambda: g(1, 2, a=3),  # positional-only passed as keyword
    lambda: k(1),  # missing keyword-only
    lambda: k(1, 2),  # too many positional (kwonly present)
    lambda: k(1, b=2, c=3),  # unexpected keyword
]
for e in errors:
    try:
        print(e())
    except TypeError as exc:
        print("TypeError:", exc)


# --- Generator parameters bind into the frame the same way ---
def gen(n, start=0, step=1):
    i = start
    while i < n:
        yield i
        i += step


print(list(gen(5)))
print(list(gen(10, 2, 3)))
print(list(gen(start=1, n=4)))
