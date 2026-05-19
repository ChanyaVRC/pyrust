# Parity tests for class bodies reading enclosing function locals via closure.
# Issue #577: class bodies that reference enclosing function locals should
# work the same as in CPython 3.12 (the compiler must promote those locals to
# cell vars in the enclosing function so the env chain carries them).

# --- basic single-level capture ---

def make_class(x):
    class C:
        value = x
    return C

print(make_class(42).value)   # 42
print(make_class("hi").value) # hi

# --- enclosing local used in expression inside class body ---

def outer_expr(n):
    class Inner:
        items = list(range(n))
    return Inner

print(outer_expr(3).items)    # [0, 1, 2]
print(outer_expr(0).items)    # []

# --- enclosing local NOT used in class body — no regression ---

def outer_no_use():
    x = 10
    class C:
        val = 5
    return C

print(outer_no_use().val)     # 5

# --- enclosing local shadows module-level global ---

z = 99

def outer_shadow():
    z = 42
    class C:
        w = z
    return C

print(outer_shadow().w)       # 42  (function-local, not module-global)
print(z)                       # 99  (module global unchanged)

# --- two different enclosing locals ---

def outer_two(a, b):
    class C:
        first = a
        second = b
    return C

cls = outer_two(1, 2)
print(cls.first)   # 1
print(cls.second)  # 2

# --- multi-level: function -> function -> class ---

def level_a(x):
    def level_b(y):
        class C:
            v = x + y
        return C
    return level_b

print(level_a(1)(2).v)    # 3
print(level_a(10)(5).v)   # 15

# --- class at module level (no enclosing function) — no regression ---

class ModLevel:
    pass

print(ModLevel.__name__)   # ModLevel

# --- enclosing local used with class-level assignment ---

def outer_mixed(n):
    extra = "x"
    class C:
        count = n
        tag = extra
        local_only = 99
    return C

c = outer_mixed(7)
print(c.count)       # 7
print(c.tag)         # x
print(c.local_only)  # 99
