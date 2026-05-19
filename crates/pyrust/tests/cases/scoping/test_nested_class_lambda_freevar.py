# Parity fixture: lambda in a nested class reads a variable from the enclosing
# function scope (issue #703).
#
# Python class scope is not a closure scope — lambdas (and methods) inside a
# class body skip the class namespace and read directly from the enclosing
# function/module scope.  When multiple levels of classes are nested, the
# compiler must recurse into each level to find free-variable reads that should
# promote the outer function's local to a cell var.

# ── Case 1: function > A > B > lambda reads outer function var ────────────────
def make_nested_1():
    x = 42
    class A:
        class B:
            fn = lambda self: x
    return A.B().fn()

print(make_nested_1())  # 42

# ── Case 2: triple nesting function > A > B > C > lambda ─────────────────────
def make_nested_2():
    x = 99
    class A:
        class B:
            class C:
                fn = lambda self: x
    return A.B.C().fn()

print(make_nested_2())  # 99

# ── Case 3: mix — method in outer class, lambda in inner class, both work ─────
def make_nested_3():
    x = 7
    class A:
        def method(self):
            return x
        class B:
            fn = lambda self: x
    return A().method(), A.B().fn()

a, b = make_nested_3()
print(a)  # 7
print(b)  # 7

# ── Case 4: module-level class with lambda — unaffected by the nested fix ─────
_module_y = 55
class _ModuleLevel:
    fn = lambda self: _module_y

print(_ModuleLevel().fn())  # 55

# ── Case 5: single class with lambda reads outer function var ─────────────────
def make_nested_5():
    x = 42
    class A:
        fn = lambda self: x
    return A().fn()

print(make_nested_5())  # 42

# ── Case 6: multiple free variables read from lambda in deeply nested class ───
def make_nested_6():
    a = 1
    b = 2
    class A:
        class B:
            fn = lambda self: a + b
    return A.B().fn()

print(make_nested_6())  # 3

# ── Case 7: lambda in inner class, method in outer class, different vars ──────
def make_nested_7():
    p = 10
    q = 20
    class A:
        def get_p(self):
            return p
        class B:
            get_q = lambda self: q
    return A().get_p(), A.B().get_q()

p_val, q_val = make_nested_7()
print(p_val)  # 10
print(q_val)  # 20
