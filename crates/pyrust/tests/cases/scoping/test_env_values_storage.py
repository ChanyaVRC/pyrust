# Parity fixture for issue #452 (closure cell storage stage).
#
# `Environment.values` was changed from a per-scope `HashMap<String, Value>`
# to a compact `EnvValues` (an inline small-vector for the common closure
# capture that promotes to a hashed map past a threshold).  These cases
# exercise every path that flows through that store: single- and multi-cell
# closures (the inline form and its promotion to a map), nonlocal read/write
# across nesting levels, sibling cells sharing a parent, `del` of a cell var,
# UnboundLocalError wording, generators capturing a cell over a suspension,
# class-body free-variable capture, walrus promotion, and the module-scope
# many-globals case that forces a promotion.

# --- single-cell closure (inline, no spill) ---
def make_adder(n):
    def add(x):
        return x + n
    return add

print(make_adder(10)(5))   # 15
print(make_adder(-3)(3))   # 0

# --- two-cell closure (still inline at the chosen threshold) ---
def make_pair(a, b):
    def both():
        return (a, b)
    return both

print(make_pair(1, 2)())   # (1, 2)

# --- many-cell closure: forces inline -> map promotion ---
def make_sum6():
    a1, a2, a3, a4, a5, a6 = 1, 2, 3, 4, 5, 6
    def inner():
        return a1 + a2 + a3 + a4 + a5 + a6
    return inner

print(make_sum6()())       # 21

# --- nonlocal write across multiple nesting levels ---
def three_levels():
    x = 1
    def mid():
        def deep():
            nonlocal x
            x += 10
        deep()
    mid()
    return x

print(three_levels())      # 11

# --- two sibling closures share one parent cell ---
def siblings():
    count = 0
    def inc():
        nonlocal count
        count += 1
    def get():
        return count
    inc(); inc(); inc()
    return get()

print(siblings())          # 3

# --- del of a cell var leaves a free-variable read raising NameError ---
def del_cell():
    y = 5
    def reader():
        return y
    del y
    try:
        reader()
    except NameError:
        return "NameError"
    return "no-error"

print(del_cell())          # NameError

# --- UnboundLocalError wording for a read-before-assignment local ---
def unbound():
    try:
        return z          # noqa: F821
        z = 1             # makes z a local
    except UnboundLocalError as e:
        return str(e)

print(unbound())
# cannot access local variable 'z' where it is not associated with a value

# --- generator capturing a cell across a suspension point ---
def gen_with_cell(start):
    i = start
    def bump():
        nonlocal i
        i += 100
    yield i
    bump()
    yield i
    bump()
    yield i

print(list(gen_with_cell(0)))   # [0, 100, 200]

# --- class body free-variable capture from the enclosing function ---
def make_class():
    val = 42
    class C:
        x = val
        def m(self):
            return val
    return C

C = make_class()
print(C.x, C().m())        # 42 42

# --- walrus inside a comprehension (PEP 572 capture) ---
print([(n := v, n * 2) for v in range(3)])
# [(0, 0), (1, 2), (2, 4)]

# --- many module globals: forces the module env to promote to a map ---
g0 = 0; g1 = 1; g2 = 2; g3 = 3; g4 = 4
g5 = 5; g6 = 6; g7 = 7; g8 = 8; g9 = 9

def read_globals():
    return g0 + g1 + g2 + g3 + g4 + g5 + g6 + g7 + g8 + g9

print(read_globals())      # 45

# globals() still exposes them as a live dict
print(globals()["g7"])     # 7
