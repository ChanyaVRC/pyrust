# Regression test for issue #699:
# A lambda inside a class body that reads a free variable whose name matches a
# class attribute must not cause that attribute to be stripped.
#
# Python class scope is not a closure scope for lambdas; free-var reads in a
# lambda match the enclosing function/module scope, never the class namespace.

# ── Case 1: basic repro ────────────────────────────────────────────────────────
x = 100
class A:
    x = 10
    fn = lambda self: x   # free var 'x' matches class attr 'x'

print(A.x)       # 10  (class attribute must survive)
print(A().fn())  # 100 (lambda reads module-level x)

# ── Case 2: lambda reads module name NOT shadowed by class attr ───────────────
y = 200
class B:
    z = 20
    fn = lambda self: y   # no collision: 'y' is not a class attr

print(B.z)       # 20
print(B().fn())  # 200

# ── Case 3: multiple names, some collide with class attrs ─────────────────────
w = 300
v = 400
class C:
    w = 30
    fn = lambda self: (w, v)   # 'w' collides, 'v' does not

print(C.w)          # 30
print(C().fn())     # (300, 400)

# ── Case 4: lambda param shadows the free-var name ───────────────────────────
x2 = 500
class D:
    x2 = 50
    fn = lambda self, x2: x2   # param 'x2' shadows the outer free var

print(D.x2)        # 50
print(D().fn(99))  # 99

# ── Case 5: lambda inside a regular function still captures correctly ─────────
def outer():
    q = 42
    fn = lambda: q   # must still close over 'q' from outer()
    return fn()

print(outer())  # 42

# ── Case 6: lambda in a method (non-class scope) captures method local ────────
class E:
    def method(self):
        local = 77
        fn = lambda: local
        return fn()

print(E().method())  # 77

# ── Case 7: class inside a function — lambda reads enclosing function local ───
def make_class():
    x = 100
    class F:
        x = 10
        fn = lambda self: x   # reads enclosing function's x, not class attr
    return F

F = make_class()
print(F.x)       # 10  (class attribute must survive)
print(F().fn())  # 100 (lambda reads enclosing function's x)

# ── Case 8: class inside a function, multiple lambdas, some name collisions ───
def make_class2():
    a = 1
    b = 2
    class G:
        a = 10
        fn1 = lambda self: a       # collides with class attr 'a'
        fn2 = lambda self: b       # no collision
    return G

G = make_class2()
print(G.a)        # 10
print(G().fn1())  # 1
print(G().fn2())  # 2
