# Parity fixture for issue #748: annotated names that are also global/nonlocal
# must raise SyntaxError at compile time.
#
# The SyntaxError cases (annotated name with global/nonlocal) are tested via
# Rust unit tests in the pyrust crate, not here, because those scripts exit
# non-zero — which the parity harness treats as a harness failure rather than
# parity evidence.
#
# This fixture verifies only the NON-conflicting cases where both CPython 3.12
# and pyrust must succeed and produce identical output.

# global without any bare annotation is fine
g = 0

def set_g():
    global g
    g = 42

set_g()
print(g)  # 42

# bare annotation without global/nonlocal is fine
def k():
    z: int
    return 0

print(k())  # 0

# nonlocal without annotation is fine
def outer():
    x = 1
    def inner():
        nonlocal x
        x = 2
    inner()
    return x

print(outer())  # 2

# annotated local with a value (not bare) is fine
def annotated_local():
    a: int = 10
    return a

print(annotated_local())  # 10

# bare annotation in a class body is fine (class has its own scope,
# separate from any enclosing function's global/nonlocal)
class C:
    y: int  # class-level bare annotation — OK

print("annotation_global_nonlocal OK")
