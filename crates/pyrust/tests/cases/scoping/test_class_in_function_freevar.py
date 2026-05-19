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
