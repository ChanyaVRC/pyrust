# Issue #708: nonlocal in class body
#
# Case 1: SyntaxError when no enclosing function binding exists.
# Case 2: nonlocal in class body updates the enclosing function's binding.

# Case 2 (basic): nonlocal x in class body reaches enclosing function
def outer_basic():
    x = 1
    class C:
        nonlocal x
        x = 2
    return x

print(outer_basic())  # 2

# Case 2 (read before write): read the nonlocal value inside class body
def outer_read():
    x = 42
    class C:
        nonlocal x
        val = x
        x = 99
    return x

print(outer_read())  # 99

# Case 2 (multiple names): nonlocal with two names
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

# Case 2 (nonlocal name not in class attrs): `x` is not added to class namespace
def outer_no_attr():
    x = 0
    class C:
        nonlocal x
        x = 5
        other = 7
    return x, hasattr(C, 'x'), C.other

r, has_x, o = outer_no_attr()
print(r, has_x, o)  # 5 False 7

# Case 2 (doubly-nested class): nonlocal in inner class reaches outer function
def outer_nested():
    x = 1
    class C:
        class D:
            nonlocal x
            x = 3
    return x

print(outer_nested())  # 3

# Case 2 (class attrs unaffected): non-nonlocal names still become class attrs
def outer_mixed():
    x = 1
    class C:
        nonlocal x
        x = 5
        y = 10
    return x, C.y

r, a = outer_mixed()
print(r, a)  # 5 10

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
