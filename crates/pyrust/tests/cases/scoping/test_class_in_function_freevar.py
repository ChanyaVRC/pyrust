# Test that a class nested inside a function correctly captures the enclosing
# function's variables when a method reads a free variable whose name matches
# a class attribute (class-in-function variant of issue #695).
#
# Python class scope is not a closure scope for methods: a method reading `x`
# skips the class body and reads from the enclosing function (or module) scope.

# --- Main repro: class in function, method reads outer var matching class attr ---
def outer():
    x = 42
    class MyClass:
        x = "class_x"
        def method(self):
            return x  # outer's x=42, not class attr
    return MyClass.x, MyClass().method()
print(outer())  # ('class_x', 42)

# --- __init__ reading outer var with same name as class attr ---
def outer2():
    val = 100
    class MyClass:
        val = 999
        def __init__(self):
            self.result = val  # outer's val=100
    return MyClass().result
print(outer2())  # 100

# --- Multiple free vars, one with name collision ---
def outer3():
    a = 1
    b = 2
    class C:
        a = 10
        def m(self):
            return a + b  # a from outer (1), b from outer (2)
    return C().m(), C.a
print(outer3())  # (3, 10)

# --- Triply nested classes: methods see the outermost function scope ---
def outer4():
    x = 111
    class A:
        x = "A"
        class B:
            x = "B"
            class C:
                x = "C"
                def method(self):
                    return x  # outer's x=111
    return A.x, A.B.x, A.B.C.x, A.B.C().method()
print(outer4())  # ('A', 'B', 'C', 111)

# --- Method-local variable shadows both class attr and outer ---
def outer5():
    x = 42
    class C:
        x = "class_x"
        def method(self):
            x = "local"
            return x
    return C.x, C().method()
print(outer5())  # ('class_x', 'local')

# --- Class in function, multiple methods with different free vars ---
def outer6():
    x = 10
    y = 20
    class A:
        x = "ax"
        y = "ay"
        def mx(self):
            return x  # outer's x=10
        def my(self):
            return y  # outer's y=20
    return A.x, A.y, A().mx(), A().my()
print(outer6())  # ('ax', 'ay', 10, 20)

# Tests for free-variable lookup in class methods defined inside functions (issue #700).
#
# Python class scope is not a closure scope for methods.  When a method reads a
# name, it skips the class namespace entirely and looks in the enclosing function
# (or module) scope.  When the class also defines an attribute with the same
# name, the outer function's value must still be reachable by the method, and
# the class attribute must remain intact.

# --- Basic case: outer function var shadowed by class attr ---

def outer_basic():
    w = 55
    class Inner:
        w = 11
        def method(self):
            return w  # must see outer function's w=55, not class w=11
    return Inner.w, Inner().method()

print(outer_basic())  # (11, 55)


# --- No name collision: regression guard ---

def outer_no_collision():
    x = 99
    class Inner:
        y = 22
        def method(self):
            return x
    return Inner.y, Inner().method()

print(outer_no_collision())  # (22, 99)


# --- Class attr only (no method reads outer var) ---

def outer_attr_only():
    w = 55
    class Inner:
        w = 11
    return Inner.w  # class attr must be 11, not 55

print(outer_attr_only())  # 11


# --- Multiple colliding names ---

def outer_multi():
    a = 10
    b = 20
    class Inner:
        a = 100  # collides with outer's a
        def method(self):
            return a, b  # a -> outer's 10; b -> outer's 20
    return Inner.a, Inner().method()

print(outer_multi())  # (100, (10, 20))


# --- Method reads a name that is NOT an outer function var (module-level) ---

module_z = 88

def outer_module_global():
    class Inner:
        module_z = 33
        def method(self):
            return module_z  # module_z=88, class's module_z=33 is skipped
    return Inner.module_z, Inner().method()

print(outer_module_global())  # (33, 88)


# --- Doubly nested: function > class > inner-class > method ---

def outer_double():
    z = 77
    class Outer:
        z = 44
        class Inner:
            z = 22
            def method(self):
                return z  # must see outer_double's z=77
    return Outer.z, Outer.Inner.z, Outer.Inner().method()

print(outer_double())  # (44, 22, 77)


# --- global declaration in method bypasses both class and function scopes ---

g = 999

def outer_global_method():
    g = 42  # local, shadows module g
    class Inner:
        g = 100
        def method(self):
            global g
            return g  # must see module g=999
    return Inner.g, Inner().method()

print(outer_global_method())  # (100, 999)


# --- Method defined inside class-level try block ---

def outer_try():
    x = 10
    class C:
        x = 99
        try:
            def method(self):
                return x  # must see outer's x=10, not class x=99
        except Exception:
            pass
    return C.x, C().method()

print(outer_try())  # (99, 10)


# --- Method defined inside class-level for loop body ---

def outer_for():
    x = 10
    class C:
        x = 99
        for _ in range(1):
            def method(self):
                return x  # must see outer's x=10
    return C.x, C().method()

print(outer_for())  # (99, 10)
