# Parity fixture for issue #1281: global/nonlocal declarations that appear
# after a prior use or assignment of the same name raise SyntaxError.
#
# The SyntaxError cases exit with code 1 and cannot be exercised inline in a
# parity fixture (the harness requires exit 0 from both interpreters).
# Those cases are covered by unit tests in interpreter/tests.rs.
#
# This fixture verifies the positive complement: valid global/nonlocal
# declarations (appearing BEFORE any use or assignment of the name) continue
# to work correctly, including edge cases adjacent to the ordering check.

# global declared before write — ok
_g = 0
def valid_global_write():
    global _g
    _g = 42
valid_global_write()
print(_g)  # 42

# global declared before read — ok
_h = 99
def valid_global_read():
    global _h
    return _h
print(valid_global_read())  # 99

# nonlocal declared before assignment — ok
def valid_nonlocal():
    counter = 0
    def inc():
        nonlocal counter
        counter += 1
    inc()
    inc()
    return counter
print(valid_nonlocal())  # 2

# nonlocal declared before read — ok
def valid_nonlocal_read():
    val = 7
    def reader():
        nonlocal val
        return val
    return reader()
print(valid_nonlocal_read())  # 7

# Nested scope assignment does NOT trigger outer scope's ordering check.
# `z = 99` is in the inner function scope, not the outer — the outer's
# `global z` should remain valid.
_z = 0
def outer_no_pollution():
    def inner():
        z = 99  # inner scope binding, does not affect outer check
    global _z
    _z = 5
outer_no_pollution()
print(_z)  # 5

# import before global is NOT an error in CPython — import statements are
# explicitly excluded from the ordering check.
def import_then_global():
    import os
    global os  # ok: import doesn't count as an assignment for this check
import_then_global()

# Duplicate global declarations (no prior use) are fine.
_dup = 10
def dup_global():
    global _dup
    global _dup
    _dup = 20
dup_global()
print(_dup)  # 20

# global declared before assignment in nested if/for — all valid when
# the global comes first.
_counter = 0
def global_before_nested_assign():
    global _counter
    for i in range(3):
        _counter += i
global_before_nested_assign()
print(_counter)  # 0+1+2 = 3

# nonlocal across multiple levels — the outermost binding is valid as long
# as nonlocal appears before any use in its own scope.
def multi_level():
    x = 0
    def level1():
        def level2():
            nonlocal x
            x += 10
        level2()
    level1()
    return x
print(multi_level())  # 10
