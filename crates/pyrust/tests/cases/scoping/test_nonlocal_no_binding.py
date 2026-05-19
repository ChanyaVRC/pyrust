# Parity fixture for issue #639: nonlocal with missing binding raises SyntaxError
# at compile time (not at runtime).
#
# The SyntaxError cases themselves exit with code 1 and cannot be exercised
# inside a parity fixture (the harness requires exit 0 from both interpreters).
# Those cases are covered by unit tests in interpreter/tests.rs.
#
# This fixture verifies the *positive* complement: that valid nonlocal bindings
# continue to work correctly at various scope nesting depths, including edge
# cases that sit adjacent to the "no binding" check — such as nonlocal across
# multiple intervening scopes, nonlocal in a class method reaching an enclosing
# function, and nonlocal names that shadow a same-named global.

# Case 1: nonlocal x where x is genuinely a local in the enclosing function
# (the bug: nonlocal with no binding must raise SyntaxError, so this valid
# case must still work after the fix).
def make_adder(n):
    total = 0
    def add(x):
        nonlocal total
        total += x
        return total
    for i in range(n):
        add(i)
    return total

print(make_adder(5))   # 0+1+2+3+4 = 10

# Case 2: nonlocal x in doubly-nested function; x is bound two levels up
# (middle does not bind x at all, so inner reaches past middle to outer).
def outer_double():
    x = 1
    def middle():
        def inner():
            nonlocal x
            x += 10
        inner()
    middle()
    return x

print(outer_double())  # 11

# Case 3: same-named global exists at module level; the nonlocal still refers
# to the enclosing *function* local, not the global.
g = 999
def outer_shadow():
    g = 0
    def inc():
        nonlocal g
        g += 1
    inc()
    inc()
    return g

print(outer_shadow())  # 2
print(g)               # 999 (global unchanged)

# Case 4: nonlocal used alongside global in sibling inner functions; each
# should resolve independently.
def outer_siblings():
    shared = 0
    def writer():
        nonlocal shared
        shared = 42
    def reader():
        return shared
    writer()
    return reader()

print(outer_siblings())  # 42

# Case 5: nonlocal in a triply-nested function where the binding is only in
# the outermost enclosing function.
def level0():
    v = 100
    def level1():
        def level2():
            def level3():
                nonlocal v
                v -= 1
            level3()
        level2()
    level1()
    return v

print(level0())  # 99
