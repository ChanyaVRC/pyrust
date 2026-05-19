# Parity fixture for issue #707: nonlocal at module level is a SyntaxError.
#
# The SyntaxError case cannot be exercised through the parity harness
# (a directly-failing script would be counted as "Python run failed").
# This fixture instead validates that nonlocal continues to work correctly
# in the valid positions — functions defined at module scope — to confirm
# that the module-level guard in compile_stmt does not regress the happy path.

# nonlocal inside a function at module level — valid.
def counter():
    n = 0
    def inc():
        nonlocal n
        n += 1
        return n
    return inc

f = counter()
print(f())   # 1
print(f())   # 2
print(f())   # 3

# nonlocal inside a nested function inside a conditional — valid.
def make_adder(start):
    total = start
    def add(x):
        nonlocal total
        total += x
        return total
    return add

g = make_adder(10)
print(g(5))   # 15
print(g(3))   # 18
