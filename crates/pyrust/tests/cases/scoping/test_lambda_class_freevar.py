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

# ── Case 9: nested class + lambda — outer class attr must not be stripped ────
# A lambda inside Inner that reads module-level 'x' must not cause Outer.x to
# be promoted to a cell var (which would strip the Outer class attribute).
x3 = 99
class Outer9:
    x3 = 10
    class Inner9:
        x3 = 20
        fn = lambda self: x3  # reads module x3, not class attrs

print(Outer9.x3)          # 10  (Outer attr must survive)
print(Outer9.Inner9.x3)   # 20  (Inner attr must survive)
print(Outer9.Inner9().fn())  # 99 (lambda reads module x3)

# ── Case 10: a nested closure in a lambda default keeps its provenance ────────
# Lambda parameters shadow names only in the body.  The default is evaluated in
# the enclosing scope, so subtracting the parameter name from default reads
# would incorrectly lose this cell dependency.
def lambda_default_capture():
    x = 123
    return (lambda x=(lambda: x): x)()()

print(lambda_default_capture())  # 123

# ── Case 11: the same default capture across a class boundary ─────────────────
# The nested lambda skips the class namespace and closes over the function's x.
def class_lambda_default_capture():
    x = 456
    class H:
        fn = lambda value=(lambda: x): value
    return H.fn()()

print(class_lambda_default_capture())  # 456

# ── Case 12: a lambda in a nested definition header is still outer-scope code ─
def definition_header_capture():
    x = 789
    def nested(callback=(lambda: x)):
        return callback()
    return nested()

print(definition_header_capture())  # 789
