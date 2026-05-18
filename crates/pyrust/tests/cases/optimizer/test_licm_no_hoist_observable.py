# Regression for #580: LICM must not hoist instructions out of zero-trip loops.
#
# Two categories of observable violations were fixed:
#   1. LoadConst for named locals: hoisting unconditionally assigns the variable
#      even when the loop body never runs.
#   2. UnaryOp / BinOpConst with exception-raising operators: hoisting moves
#      the raise point before the loop header, firing in zero-trip loops where
#      CPython 3.12 never enters the body.

# ── Case 1: LoadConst for a named local must not be hoisted ──────────────────
# range(0) → zero iterations; flag must remain False.
flag = False
for i in range(0):
    flag = True
assert flag == False, f"flag should be False, got {flag!r}"
print("case1:", flag)

# Nested variant: inner loop is zero-trip; outer modifies the local.
x = 0
for j in range(3):
    for i in range(0):
        x = 99
    x += 1
assert x == 3, f"x should be 3, got {x!r}"
print("case1b:", x)

# ── Case 2: UnaryOp must not be hoisted (can raise TypeError) ────────────────
# range(0) → zero iterations; "+string" TypeError must NOT fire.
s = "hello"
for i in range(0):
    y = +s  # TypeError: bad operand type for unary + — must never execute
print("case2: done")

# ── Case 3: BinOpConst(FloorDiv, 0) must not be hoisted ─────────────────────
# range(0) → zero iterations; "x // 0" ZeroDivisionError must NOT fire.
val = 42
for i in range(0):
    z = val // 0  # ZeroDivisionError — must never execute
print("case3: done")

# ── Case 4: BinOpConst(LShift, negative) must not be hoisted ─────────────────
# range(0) → zero iterations; "x << -1" ValueError must NOT fire.
val = 1
for i in range(0):
    w = val << -1  # ValueError — must never execute
print("case4: done")

# ── Case 5: LoadConst for temps is still hoisted (regression check) ───────────
# A loop that runs N > 0 times with a body-local constant temp should still
# benefit from LICM.  We verify correctness, not that hoisting happened.
total = 0
for i in range(5):
    total += 10
assert total == 50, f"total should be 50, got {total!r}"
print("case5:", total)

# ── Case 6: BinOpConst(Add) for a temp is still hoisted (regression check) ───
# Same as above but with an explicit constant expression.
acc = 0
base = 7
for i in range(4):
    acc += base + 3  # BinOpConst(Add, 3) with base not written → should hoist
assert acc == 40, f"acc should be 40, got {acc!r}"
print("case6:", acc)

print("licm_no_hoist_observable ok")
