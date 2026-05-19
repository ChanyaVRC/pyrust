# Parity fixture for issue #725: dead-code branches must still be subject to
# compile-time syntax checks (`break`/`continue` outside loop, `nonlocal` at
# module level).
#
# The invalid forms (e.g. `if False: break`) are SyntaxErrors that cause the
# script to exit non-zero; parity harness scripts must exit 0, so the invalid
# cases cannot appear here directly.  They are validated via unit tests in
# crates/pyrust/tests/cases that explicitly confirm exit-code-1 + SyntaxError
# output.  This fixture validates the *valid* counterparts — paths that must
# continue to work correctly after the fix.

# Valid: break inside a loop that is itself inside an always-false if.
# The break targets the for-loop, which is a valid enclosing loop.
if False:
    for i in range(10):
        break   # valid — inside a loop

# Valid: continue inside a loop inside an always-false branch.
if False:
    for i in range(10):
        continue   # valid — inside a loop

# Valid: while False body — the while itself is a loop context, so break and
# continue inside it refer to that loop and are syntactically valid.
while False:
    break     # valid — the while is the enclosing loop
while False:
    continue  # valid — the while is the enclosing loop

# Valid: break and continue inside a nested loop in an always-true else-branch
# of an if-True construct.  The else clause is dead code but its nested loop
# is still a valid enclosing loop for break/continue.
if True:
    x = 1
else:
    for i in range(10):
        break    # valid — inside a loop, even though the else is dead

print(x)   # 1 — if True took the live branch

# Valid: nonlocal inside a function body that is nested in a dead-code branch.
# Def bodies have independent scope rules; the nonlocal in `inner` is checked
# against `outer`'s locals, not the module scope.
def outer():
    n = 0
    def inner():
        if False:
            pass   # dead branch; nonlocal not present here at module level
        nonlocal n
        n += 1
        return n
    return inner

f = outer()
print(f())   # 1
print(f())   # 2

# Valid: nonlocal inside a function where an always-false branch precedes it.
def make_adder(start):
    total = start
    def add(x):
        if False:
            pass   # dead branch
        nonlocal total
        total += x
        return total
    return add

g = make_adder(10)
print(g(5))   # 15
print(g(3))   # 18

# Valid: break/continue inside loops that appear inside always-true branches.
result = []
for outer_i in range(3):
    if True:
        for inner_j in range(3):
            if inner_j == 1:
                break   # valid — inside inner for-loop
    result.append(outer_i)
print(result)   # [0, 1, 2]
