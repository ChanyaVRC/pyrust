# Parity fixture for issue #1105: nonlocal with no enclosing binding must raise
# SyntaxError at compile time (not RuntimeError at runtime).
#
# The SyntaxError cases (scripts that contain an invalid nonlocal) exit with
# code 1 and cannot be exercised directly in the parity harness (which requires
# exit 0 from both interpreters).  Those cases are covered by unit tests in
# crates/pyrust/src/interpreter/tests.rs.
#
# This fixture verifies the *positive* paths: valid nonlocal bindings continue
# to work correctly, including the patterns adjacent to the invalid case.

# Basic: inner function reads and mutates an enclosing binding.
def outer():
    x = 10
    def inner():
        nonlocal x
        x = 20
    inner()
    return x

print(outer())   # 20

# The error is raised at compile time (before any code runs).
# Verify the module-level print executes normally when nonlocal is valid.
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

# Nonlocal works across multiple intervening scopes.
def level0():
    v = 100
    def level1():
        def level2():
            nonlocal v
            v -= 1
        level2()
    level1()
    return v

print(level0())  # 99

# Nonlocal in a class method reaches the enclosing function (class scope is
# transparent to nonlocal; issue #633 / #735).
def outer_class():
    x = 1
    class C:
        def method(self):
            nonlocal x
            x = 2
    C().method()
    return x

print(outer_class())  # 2

print("nonlocal OK")
