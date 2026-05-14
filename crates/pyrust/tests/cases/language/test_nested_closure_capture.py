# Closure capture across three or more nesting levels.
#
# Regression coverage for issue #381: an inner function must be able to read a
# variable defined in an outer enclosing scope even when the intermediate scope
# does not reference that variable itself.  The compiler now collects free-var
# reads transitively through nested function/class/lambda bodies so the
# enclosing scope promotes the binding to a cell var.

# ----------------------------------------------------------------------------
# Three-level direct read (the original repro).
# ----------------------------------------------------------------------------
def outer():
    x = 1
    def middle():
        def inner():
            return x
        return inner()
    return middle()

assert outer() == 1
print("3-level direct read:", outer())


# ----------------------------------------------------------------------------
# Four levels deep — make sure the propagation handles arbitrary depth.
# ----------------------------------------------------------------------------
def a():
    v = "hi"
    def b():
        def c():
            def d():
                return v
            return d()
        return c()
    return b()

assert a() == "hi"
print("4-level read:", a())


# ----------------------------------------------------------------------------
# Three-level write through `nonlocal` — innermost function writes a binding
# owned by the outermost.
# ----------------------------------------------------------------------------
def make_counter():
    n = 0
    def step():
        nonlocal n
        def bump():
            nonlocal n
            n += 1
        bump()
        return n
    return step

c = make_counter()
assert c() == 1
assert c() == 2
print("counter:", c())


# ----------------------------------------------------------------------------
# Parameterised decorator — the canonical real-world case.
# ----------------------------------------------------------------------------
def trace(name):
    def deco(fn):
        def wrap(*a, **kw):
            result = fn(*a, **kw)
            return f"trace-{name}/{result}"
        return wrap
    return deco

@trace("outer")
def traced():
    return 1

assert traced() == "trace-outer/1"
print("decorator:", traced())


# ----------------------------------------------------------------------------
# Issue's exact final example.
# ----------------------------------------------------------------------------
def g(x):
    def h():
        def i():
            return x
        return i
    return h

assert g(7)()() == 7
print("issue repro:", g(7)()())


# ----------------------------------------------------------------------------
# Explicit `nonlocal` at the innermost level — the middle scope never names x
# itself, so the compiler must still treat x as a cell var in `outer`.
# ----------------------------------------------------------------------------
def outer_nl(x):
    def mid():
        def inner():
            nonlocal x
            return x
        return inner
    return mid

assert outer_nl(99)()() == 99
print("nonlocal-3:", outer_nl(99)()())


# ----------------------------------------------------------------------------
# Inner scope shadows the outer binding — the shadow must win.
# ----------------------------------------------------------------------------
def shadow():
    x = 1
    def middle():
        x = 99
        def inner():
            return x
        return inner()
    return middle()

assert shadow() == 99
print("shadow:", shadow())


# ----------------------------------------------------------------------------
# Middle scope reads x AND inner scope reads x — the previously-working
# 2-level case must still work alongside the new transitive case.
# ----------------------------------------------------------------------------
def mixed():
    x = "v"
    def middle():
        y = x + "/mid"
        def inner():
            return x + "/inner"
        return (y, inner())
    return middle()

assert mixed() == ("v/mid", "v/inner")
print("mixed:", mixed())


# ----------------------------------------------------------------------------
# Single-level closure — regression guard for the path that already worked.
# ----------------------------------------------------------------------------
def adder(x):
    def add(y):
        return x + y
    return add

assert adder(3)(4) == 7
print("single-level:", adder(3)(4))


# ----------------------------------------------------------------------------
# Lambda nested two levels deep reads an outer fastlocal.
# ----------------------------------------------------------------------------
def lam_outer():
    z = 100
    def middle():
        return (lambda: z)()
    return middle()

assert lam_outer() == 100
print("lambda-3:", lam_outer())


# ----------------------------------------------------------------------------
# Class defined inside a function whose method reads an enclosing fastlocal
# via two scope levels (function -> class -> method).
# ----------------------------------------------------------------------------
def cls_outer():
    val = 5
    def middle():
        class C:
            def m(self):
                return val
        return C().m()
    return middle()

assert cls_outer() == 5
print("class-in-fn:", cls_outer())


# ----------------------------------------------------------------------------
# Default-value expressions for a deeply nested function read outer fastlocals.
# Defaults evaluate in the enclosing scope, so this must work just like a
# regular free-var read.
# ----------------------------------------------------------------------------
def defaults_outer():
    base = 10
    def middle():
        def inner(off=base):
            return off
        return inner()
    return middle()

assert defaults_outer() == 10
print("default-3:", defaults_outer())


# ----------------------------------------------------------------------------
# Each call to the outer function gets a fresh binding — closures must see
# the binding from their defining call frame, not a stale one.
# ----------------------------------------------------------------------------
def factory(x):
    def mid():
        def inner():
            return x
        return inner
    return mid

f1 = factory(1)()
f2 = factory(2)()
assert f1() == 1
assert f2() == 2
print("fresh-binding:", f1(), f2())
