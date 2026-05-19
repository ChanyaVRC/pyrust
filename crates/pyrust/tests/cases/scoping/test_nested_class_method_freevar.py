# Parity fixture for issue #690:
# A method inside a nested class that reads a free variable whose name matches
# an outer class attribute must not corrupt the outer class's attribute dict.

x = 99  # module global

class Outer:
    x = 50  # class attribute; must NOT be turned into a cell var

    class Inner:
        def method(self):
            return x  # free-variable read — refers to module global (99)

# Outer.x must still be the class attribute 50 (not corrupted to AttributeError).
print(Outer.x)  # 50
# The method's free-variable read must return the module global.
print(Outer.Inner().method())  # 99

# Basic class attributes are unaffected.
class A:
    a = 1
    b = 2
print(A.a, A.b)  # 1 2

# A flat class method reading a free variable from an enclosing function scope
# must still work (the guard must only skip the class-body-is-outer-scope case).
def outer_fn():
    val = 42
    class C:
        def method(self):
            return val  # captures val from outer_fn
    return C().method()

print(outer_fn())  # 42

# Deeper nesting: multiple levels of nested classes, each with a class attr
# that shares a name with a module global — none should be corrupted.
w = 1
class L1:
    w = 2
    class L2:
        w = 3
        class L3:
            def method(self):
                return w  # module global

print(L1.w)      # 2
print(L1.L2.w)   # 3
print(L1.L2.L3().method())  # 1

# inner class method with explicit `global` declaration must not corrupt outer
# class attribute either (regression for issue #629).
z = 100
class Outer2:
    z = 77
    class Inner2:
        def method(self):
            global z
            return z  # module global via explicit declaration

print(Outer2.z)              # 77
print(Outer2.Inner2().method())  # 100
