# Parity fixture for issue #763: annotated assignment + global/nonlocal.
#
# CPython 3.12 raises SyntaxError when an annotated name (x: T = v or x: T)
# is combined with a global or nonlocal declaration in the same function body.
# The SyntaxError cases are validated in Rust unit tests
# (interpreter/tests.rs::annotated_assign_with_global_is_syntax_error, etc.)
# because pyrust does not implement exec()/compile() and the parity harness
# requires both interpreters to exit 0.
#
# This fixture covers the valid (non-conflicting) paths that must continue to
# work after the fix.

# Annotated assignment in a function body (no global/nonlocal) — must work.
def simple_ann_assign():
    x: int = 42
    return x

print(simple_ann_assign())  # 42

# Bare annotation in a function body (no global/nonlocal) — no-op, no crash.
def bare_ann():
    x: int
    x = 7
    return x

print(bare_ann())  # 7

# Annotated assignment at module scope with no conflict.
module_var: int = 99
print(module_var)  # 99

# global declaration with a plain (non-annotated) assignment is fine.
counter = 0

def inc():
    global counter
    counter = counter + 1

inc()
inc()
print(counter)  # 2

# nonlocal declaration with a plain (non-annotated) assignment is fine.
def make_acc():
    total = 0
    def add(n):
        nonlocal total
        total = total + n
    add(3)
    add(7)
    return total

print(make_acc())  # 10

# Annotated assignment in a class body (no global/nonlocal) — must work.
class Box:
    value: int = 0

    def set(self, v: int) -> None:
        self.value = v

b = Box()
b.set(55)
print(b.value)  # 55
print(Box.value)  # 0

# Annotated assignment inside a while-index loop body.
items = [10, 20, 30]
i = 0
results = []
while i < len(items):
    val: int = items[i]
    results.append(val)
    i += 1
print(results)  # [10, 20, 30]
