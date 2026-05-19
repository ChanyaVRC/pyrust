# Issue #735: nonlocal in class body inside nested function silently returned
# old value.  Class scope is transparent to `nonlocal` — a `nonlocal x` declared
# in a class body reaches the enclosing *function* scope, even when the class is
# nested inside another function.

# Case 1: basic — nonlocal x in class body directly inside function
def outer_basic():
    x = 1
    class C:
        nonlocal x
        x = 2
    return x

print(outer_basic())  # 2

# Case 2: nonlocal x in class body inside nested function
def outer_nested_fn():
    x = 1
    def inner():
        class C:
            nonlocal x
            x = 5
    inner()
    return x

print(outer_nested_fn())  # 5

# Case 3: nonlocal at function level + class in between
def outer3():
    x = 1
    def inner():
        nonlocal x
        class C:
            pass
        x = 2
    inner()
    return x

print(outer3())  # 2

# Case 4: read nonlocal before write in class body
def outer_read():
    x = 42
    class C:
        nonlocal x
        val = x
        x = 99
    return x

print(outer_read())  # 99

# Case 5: multiple nonlocal names
def outer_multi():
    x = 1
    y = 2
    class C:
        nonlocal x, y
        x = 10
        y = 20
    return x, y

a, b = outer_multi()
print(a, b)  # 10 20

# Case 6: nonlocal name is not added to class attrs
def outer_no_attr():
    x = 0
    class C:
        nonlocal x
        x = 5
        other = 7
    return x, hasattr(C, 'x'), C.other

r, has_x, o = outer_no_attr()
print(r, has_x, o)  # 5 False 7

# Case 7: doubly-nested class bodies
def outer_doubly_nested():
    x = 1
    class C:
        class D:
            nonlocal x
            x = 3
    return x

print(outer_doubly_nested())  # 3

# Case 8: non-nonlocal class attrs are unaffected
def outer_mixed():
    x = 1
    class C:
        nonlocal x
        x = 5
        y = 10
    return x, C.y

r, a = outer_mixed()
print(r, a)  # 5 10

# Case 9: triply-nested functions with class at the bottom
def outer_triple():
    x = 1
    def inner():
        def inner2():
            class C:
                nonlocal x
                x = 99
        inner2()
    inner()
    return x

print(outer_triple())  # 99

# Case 10: nonlocal in class inside inner, inner reads updated value
def outer_inner_read():
    x = 1
    def inner():
        class C:
            nonlocal x
            x = 10
        return x
    return inner()

print(outer_inner_read())  # 10

# Regression: regular class without nonlocal still works
class Regular:
    z = 42

print(Regular.z)  # 42

# Regression: global in class body still works (issue #618)
g = 1
class WithGlobal:
    global g
    g = 99

print(g)  # 99
