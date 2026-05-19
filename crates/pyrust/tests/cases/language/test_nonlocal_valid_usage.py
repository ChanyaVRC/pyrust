# Parity fixture for issue #639: valid nonlocal usage in various patterns.
#
# Tests nonlocal where a binding exists in an enclosing function scope; both
# CPython and pyrust should exit with code 0 so the parity harness can diff
# the output.  The invalid case (nonlocal with no enclosing binding, which must
# raise SyntaxError at compile time) is tested manually because CPython and
# pyrust produce different traceback formats that the harness cannot diff.

# Single-level: inner reads and mutates outer's variable.
def outer():
    x = 1
    def inner():
        nonlocal x
        x = 2
    inner()
    return x

print(outer())  # 2

# Multiple nonlocal names, all valid.
def outer2():
    a, b = 1, 2
    def inner():
        nonlocal a, b
        a, b = b, a
    inner()
    return a, b

print(outer2())  # (2, 1)

# Doubly-nested: innermost nonlocal reaches two levels up.
def outer3():
    x = 10
    def middle():
        def inner():
            nonlocal x
            x = 20
        inner()
    middle()
    return x

print(outer3())  # 20

# nonlocal in a conditional branch still works.
def outer4():
    flag = False
    def toggle():
        nonlocal flag
        flag = not flag
    toggle()
    toggle()
    toggle()
    return flag

print(outer4())  # True

# nonlocal counter accumulator.
def make_counter():
    count = 0
    def inc():
        nonlocal count
        count += 1
        return count
    return inc

c = make_counter()
print(c())  # 1
print(c())  # 2
print(c())  # 3
